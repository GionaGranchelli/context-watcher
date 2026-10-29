package dev.contextwatcher.intellij

import com.google.gson.JsonObject
import com.google.gson.JsonParser
import com.intellij.openapi.Disposable
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.ui.EditorNotifications
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.WebSocket
import java.net.http.HttpResponse
import java.nio.file.Files
import java.nio.file.Path
import java.time.Duration
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

data class FileContextStateDto(
    val path: String,
    val freshness: String,
    val lastReadTurn: Int,
    val readCount: Int,
    val totalTokens: Int,
    val lineRanges: List<Pair<Int, Int>>
)

@Service(Service.Level.PROJECT)
class CwDaemonService(private val project: Project) : Disposable {
    private val logger = Logger.getInstance(CwDaemonService::class.java)
    private val executor = Executors.newSingleThreadScheduledExecutor()
    private val httpClient = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(3)).build()
    private val fileStates = ConcurrentHashMap<String, FileContextStateDto>()
    @Volatile var activeSessionId: String? = null
        private set
    @Volatile var connectionStatus: String = "Connecting"
        private set
    @Volatile private var selectedSessionId: String? = null
    @Volatile private var sessions: List<Pair<String, String>> = emptyList()

    @Volatile private var socket: WebSocket? = null
    init { executor.scheduleWithFixedDelay({
        if (socket == null) { connect(); refresh() }
    }, 0, 5, TimeUnit.SECONDS) }

    private fun connect() {
        try {
            socket = httpClient.newWebSocketBuilder().subprotocols("cw", "cw-token.${token()}")
                .buildAsync(URI.create("ws://127.0.0.1:4400/api/v1/ws"), object : WebSocket.Listener {
                    override fun onOpen(webSocket: WebSocket) { webSocket.request(1) }
                    override fun onText(webSocket: WebSocket, data: CharSequence, last: Boolean): java.util.concurrent.CompletionStage<*>? {
                        if (last) refreshFromDaemon()
                        webSocket.request(1)
                        return null
                    }
                    override fun onClose(webSocket: WebSocket, statusCode: Int, reason: String): java.util.concurrent.CompletionStage<*>? { socket = null; return null }
                    override fun onError(webSocket: WebSocket, error: Throwable) { socket = null }
                }).get(5, TimeUnit.SECONDS)
        } catch (error: Exception) { socket = null; logger.debug("cw socket unavailable", error) }
    }

    private fun normalize(path: String): String = Path.of(path).toAbsolutePath().normalize().toString()
    private fun token(): String = System.getenv("CW_CONTROL_TOKEN") ?: Files.readString(
        System.getenv("CW_CONTROL_TOKEN_FILE")?.let { Path.of(it) }
            ?: Path.of(System.getProperty("user.home"), ".local/share/context-watcher/control-token")
    ).trim()
    private fun request(path: String): HttpRequest.Builder = HttpRequest.newBuilder()
        .uri(URI.create("http://127.0.0.1:4400/api/v1/$path"))
        .timeout(Duration.ofSeconds(5)).header("Authorization", "Bearer ${token()}")
    private fun get(path: String): String {
        val response = httpClient.send(request(path).GET().build(), HttpResponse.BodyHandlers.ofString())
        check(response.statusCode() in 200..299) { "cw returned HTTP ${response.statusCode()}" }
        return response.body()
    }
    fun availableSessions(): List<Pair<String, String>> = sessions
    fun selectSession(id: String) { selectedSessionId = id; activeSessionId = null; refreshFromDaemon() }
    fun refreshFromDaemon() { if (!executor.isShutdown) executor.execute { refresh() } }
    fun getFileState(filePath: String): FileContextStateDto? = fileStates[normalize(filePath)]

    private fun refresh() {
        if (project.isDisposed) return
        try {
            val root = project.basePath?.let { normalize(it) } ?: return
            val candidates = JsonParser.parseString(get("sessions")).asJsonArray.filter { element ->
                val workspace = element.asJsonObject.get("workspace_root")
                workspace != null && !workspace.isJsonNull && normalize(workspace.asString) == root
            }.map { it.asJsonObject }
            sessions = candidates.map { it.get("id").asString to (it.get("title")?.takeUnless { title -> title.isJsonNull }?.asString ?: it.get("id").asString) }
            val selected = candidates.singleOrNull { it.get("id").asString == selectedSessionId }
                ?: candidates.singleOrNull()
            activeSessionId = selected?.get("id")?.asString
            if (activeSessionId == null) {
                fileStates.clear()
                connectionStatus = if (candidates.isEmpty()) "No session for this project" else "Select a project session"
                return
            }
            val parsed = JsonParser.parseString(get("sessions/$activeSessionId/files")).asJsonArray
            val next = mutableMapOf<String, FileContextStateDto>()
            for (element in parsed) {
                val value = element.asJsonObject
                val relative = value.get("path").asString
                val full = Path.of(relative).let { if (it.isAbsolute) it else Path.of(root).resolve(it) }.normalize().toString()
                val ranges = value.getAsJsonArray("line_ranges")?.map { range ->
                    range.asJsonArray[0].asInt to range.asJsonArray[1].asInt
                } ?: emptyList()
                next[full] = FileContextStateDto(relative, value.get("freshness").asString,
                    value.get("last_read_turn").asInt, value.get("read_count").asInt,
                    value.get("total_tokens_in_context").asInt, ranges)
            }
            fileStates.clear(); fileStates.putAll(next)
            connectionStatus = "Connected"
            ApplicationManager.getApplication().invokeLater {
                if (!project.isDisposed) EditorNotifications.getInstance(project).updateAllNotifications()
            }
        } catch (error: Exception) {
            activeSessionId = null
            fileStates.clear()
            connectionStatus = "cw unavailable: ${error.javaClass.simpleName}"
            logger.debug("cw refresh failed", error)
        }
    }

    fun shadowDedupeFile(filePath: String?) {
        val op = JsonObject().apply { addProperty("op", "shadow_dedupe"); addProperty("file_path", filePath) }
        postMutation(op)
    }
    fun pinSnippet(label: String, content: String) {
        val op = JsonObject().apply { addProperty("op", "pin_context"); addProperty("label", label); addProperty("content", content) }
        postMutation(op)
    }
    private fun postMutation(op: JsonObject) {
        val session = activeSessionId ?: return
        executor.execute {
            try {
                check(activeSessionId == session) { "Selected session changed" }
                val graph = JsonParser.parseString(get("sessions/$session/graph")).asJsonObject
                val body = JsonObject().apply { add("op", op); addProperty("status", "committed") }
                val builder = request("sessions/$session/mutations").header("Content-Type", "application/json")
                graph.get("revision")?.let { builder.header("If-Match", it.asString) }
                val response = httpClient.send(builder.POST(HttpRequest.BodyPublishers.ofString(body.toString())).build(), HttpResponse.BodyHandlers.ofString())
                check(response.statusCode() in 200..299) { "Mutation failed: HTTP ${response.statusCode()}" }
                refresh()
            } catch (error: Exception) {
                connectionStatus = "Mutation failed: ${error.javaClass.simpleName}"
                logger.warn("cw mutation failed", error)
            }
        }
    }
    override fun dispose() { socket?.abort(); executor.shutdownNow() }
}
