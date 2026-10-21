package dev.contextwatcher.intellij

import com.intellij.codeInsight.daemon.LineMarkerInfo
import com.intellij.codeInsight.daemon.LineMarkerProvider
import com.intellij.openapi.editor.markup.GutterIconRenderer
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.icons.AllIcons

class CwLineMarkerProvider : LineMarkerProvider {
    override fun getLineMarkerInfo(element: PsiElement): LineMarkerInfo<*>? {
        if (element !is PsiFile) return null
        val vFile = element.virtualFile ?: return null
        val service = element.project.getService(CwDaemonService::class.java)
        val state = service.getFileState(vFile.path) ?: return null

        val firstChild = element.firstChild ?: return null
        return LineMarkerInfo(
            firstChild,
            firstChild.textRange,
            AllIcons.General.InspectionsEye,
            { "contextWatcher: File in LLM Context (${state.freshness.uppercase()} • ${state.totalTokens} tokens)" },
            null,
            GutterIconRenderer.Alignment.LEFT,
            { "contextWatcher LLM Context Status" }
        )
    }
}
