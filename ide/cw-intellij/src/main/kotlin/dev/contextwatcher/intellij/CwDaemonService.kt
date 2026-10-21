package dev.contextwatcher.intellij

import com.intellij.openapi.components.Service
import com.intellij.openapi.project.Project
import com.intellij.ui.EditorNotifications
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.net.http.WebSocket
import java.util.concurrent.CompletionStage
import java.util.concurrent.ConcurrentHashMap
import java.util.regex.Pattern

data class FileContextStateDto(
    val path: String,
    val freshness: String, // "fresh" | "stale" | "duplicate" | "pinned"
    val lastReadTurn: Int,
    val readCount: Int,
    val totalTokens: Int,
    val lineRanges: List<Pair<Int, Int>>
)

@Service(Service.Level.PROJECT)
class CwDaemonService(private val project: Project) {
    private val httpClient: HttpClient = HttpClient.newHttpClient()
    private val fileStates = ConcurrentHashMap<String, FileContextStateDto>()
    @Volatile
    var activeSessionId: String? = null
        private set

    init {
        connectWebSocket()
        refreshFromDaemon()
    }

    fun getFileState(filePath: String): FileContextStateDto? {
        fileStates[filePath]?.let { return it }
        return fileStates.entries.firstOrNull { (k, _) ->
            filePath.endsWith(k) || k.endsWith(filePath)
        }?.value
    }

    fun refreshFromDaemon() {
        Thread {
            try {
                val sessReq = HttpRequest.newBuilder()
                    .uri(URI.create("http://127.0.0.1:4400/api/v1/sessions"))
                    .GET()
                    .build()
                val sessResp = httpClient.send(sessReq, HttpResponse.BodyHandlers.ofString())
                val idMatcher = Pattern.compile("\"id\"\\s*:\\s*\"([^\"]+)\"").matcher(sessResp.body())
                if (idMatcher.find()) {
                    val sessId = idMatcher.group(1)
                    activeSessionId = sessId
                    val filesReq = HttpRequest.newBuilder()
                        .uri(URI.create("http://127.0.0.1:4400/api/v1/sessions/$sessId/files"))
                        .GET()
                        .build()
                    val filesResp = httpClient.send(filesReq, HttpResponse.BodyHandlers.ofString())
                    parseFileStatesJson(filesResp.body())
                    EditorNotifications.getInstance(project).updateAllNotifications()
                }
            } catch (_: Exception) {
            }
        }.start()
    }

    fun shadowDedupeFile(filePath: String?) {
        val sessId = activeSessionId ?: return
        val pathJson = if (filePath != null) "\"$filePath\"" else "null"
        val body = """{"op":{"op":"shadow_dedupe","file_path":$pathJson},"status":"committed"}"""
        postMutation(sessId, body)
    }

    fun pinSnippet(label: String, content: String) {
        val sessId = activeSessionId ?: return
        val escaped = content.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n")
        val body = """{"op":{"op":"pin_context","label":"$label","content":"$escaped","source_node_id":null},"status":"committed"}"""
        postMutation(sessId, body)
    }

    private fun postMutation(sessId: String, jsonBody: String) {
        Thread {
            try {
                val req = HttpRequest.newBuilder()
                    .uri(URI.create("http://127.0.0.1:4400/api/v1/sessions/$sessId/mutations"))
                    .header("Content-Type", "application/json")
                    .POST(HttpRequest.BodyPublishers.ofString(jsonBody))
                    .build()
                httpClient.send(req, HttpResponse.BodyHandlers.ofString())
                refreshFromDaemon()
            } catch (_: Exception) {
            }
        }.start()
    }

    private fun connectWebSocket() {
        try {
            httpClient.newWebSocketBuilder()
                .buildAsync(URI.create("ws://127.0.0.1:4400/api/v1/ws"), object : WebSocket.Listener {
                    override fun onText(webSocket: WebSocket, data: CharSequence, last: Boolean): CompletionStage<*>? {
                        if (last) {
                            refreshFromDaemon()
                        }
                        return super.onText(webSocket, data, last)
                    }
                })
        } catch (_: Exception) {
        }
    }

    private fun parseFileStatesJson(json: String) {
        fileStates.clear()
        val objPattern = Pattern.compile(
            "\"path\"\\s*:\\s*\"([^\"]+)\".*?\"freshness\"\\s*:\\s*\"([^\"]+)\".*?\"last_read_turn\"\\s*:\\s*(\\d+).*?\"read_count\"\\s*:\\s*(\\d+).*?\"total_tokens_in_context\"\\s*:\\s*(\\d+)"
        )
        val matcher = objPattern.matcher(json)
        while (matcher.find()) {
            val path = matcher.group(1)
            val dto = FileContextStateDto(
                path = path,
                freshness = matcher.group(2),
                lastReadTurn = matcher.group(3).toIntOrNull() ?: 1,
                readCount = matcher.group(4).toIntOrNull() ?: 1,
                totalTokens = matcher.group(5).toIntOrNull() ?: 0,
                lineRanges = listOf(1 to 200)
            )
            fileStates[path] = dto
        }
    }
}
