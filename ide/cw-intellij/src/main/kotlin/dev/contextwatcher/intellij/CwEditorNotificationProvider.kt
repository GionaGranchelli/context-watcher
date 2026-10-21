package dev.contextwatcher.intellij

import com.intellij.openapi.fileEditor.FileEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.ui.EditorNotificationPanel
import com.intellij.ui.EditorNotificationProvider
import java.util.function.Function
import javax.swing.JComponent

class CwEditorNotificationProvider : EditorNotificationProvider {
    override fun collectNotificationData(
        project: Project,
        file: VirtualFile
    ): Function<in FileEditor, out JComponent?>? {
        val service = project.getService(CwDaemonService::class.java)
        val state = service.getFileState(file.path) ?: return null

        return Function { _ ->
            val panel = EditorNotificationPanel()
            when (state.freshness) {
                "stale" -> {
                    panel.text = "🟠 contextWatcher: Stale in LLM Context (Read at Turn #${state.lastReadTurn}, modified since • ${state.totalTokens} tok)"
                    panel.createActionLabel("Shadow-Dedupe / Evict Stale Read") {
                        service.shadowDedupeFile(state.path)
                    }
                }
                "duplicate" -> {
                    panel.text = "🔁 contextWatcher: Duplicate in LLM Context (Read ${state.readCount}x • ${state.totalTokens} tok)"
                    panel.createActionLabel("Deduplicate Reads") {
                        service.shadowDedupeFile(state.path)
                    }
                }
                "pinned" -> {
                    panel.text = "📌 contextWatcher: Pinned in LLM High-Attention Tail (${state.totalTokens} tok)"
                }
                else -> {
                    panel.text = "🟢 contextWatcher: Fresh in LLM Context (Turn #${state.lastReadTurn} • ${state.totalTokens} tok)"
                }
            }
            panel
        }
    }
}
