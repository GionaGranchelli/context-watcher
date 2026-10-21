package dev.contextwatcher.intellij

import com.intellij.openapi.project.Project
import com.intellij.openapi.wm.ToolWindow
import com.intellij.openapi.wm.ToolWindowFactory
import com.intellij.ui.content.ContentFactory
import com.intellij.ui.jcef.JBCefApp
import com.intellij.ui.jcef.JBCefBrowser
import java.awt.BorderLayout
import javax.swing.JButton
import javax.swing.JLabel
import javax.swing.JPanel

class CwToolWindowFactory : ToolWindowFactory {
    override fun createToolWindowContent(project: Project, toolWindow: ToolWindow) {
        val panel = JPanel(BorderLayout())
        val topBar = JPanel()
        val refreshBtn = JButton("Sync with cw Daemon")
        refreshBtn.addActionListener {
            project.getService(CwDaemonService::class.java).refreshFromDaemon()
        }
        val dedupeBtn = JButton("Shadow-Dedupe All Stale Reads")
        dedupeBtn.addActionListener {
            project.getService(CwDaemonService::class.java).shadowDedupeFile(null)
        }
        topBar.add(refreshBtn)
        topBar.add(dedupeBtn)
        panel.add(topBar, BorderLayout.NORTH)

        if (JBCefApp.isSupported()) {
            val browser = JBCefBrowser("http://127.0.0.1:4400")
            panel.add(browser.component, BorderLayout.CENTER)
        } else {
            panel.add(
                JLabel("contextWatcher Dashboard running at http://127.0.0.1:4400"),
                BorderLayout.CENTER
            )
        }

        val content = ContentFactory.getInstance().createContent(panel, "Dashboard", false)
        toolWindow.contentManager.addContent(content)
    }
}
