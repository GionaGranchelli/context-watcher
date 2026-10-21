package dev.contextwatcher.intellij

import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys

class PinSelectionToContextAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val editor = e.getData(CommonDataKeys.EDITOR) ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE)
        val selectedText = editor.selectionModel.selectedText ?: return
        val label = file?.name ?: "IDE Selection"
        project.getService(CwDaemonService::class.java).pinSnippet(label, selectedText)
    }
}

class EvictStaleFileAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE)
        project.getService(CwDaemonService::class.java).shadowDedupeFile(file?.path)
    }
}
