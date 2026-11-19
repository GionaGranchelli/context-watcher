package com.cwk.api

import java.io.File
import java.util.Base64
import org.springframework.http.HttpStatus
import org.springframework.web.bind.annotation.GetMapping
import org.springframework.web.bind.annotation.RequestParam
import org.springframework.web.bind.annotation.RestController
import org.springframework.web.server.ResponseStatusException
import tools.jackson.databind.ObjectMapper

@RestController
class BoardController(mapper: ObjectMapper) {
    data class Task(val key: String, val title: String, val status: String, val priority: String)
    data class Page(val items: List<Task>, val nextCursor: String?)

    private val rows = System.getenv("CWK_TASK_FIXTURE")?.let {
        mapper.readValue(File(it), Array<Task>::class.java).toList()
    }.orEmpty()

    @GetMapping("/api/v1/tasks")
    fun tasks(
        @RequestParam(name = "status", required = false) status: String?,
        @RequestParam(name = "priority", required = false) priority: String?,
        @RequestParam(name = "limit", defaultValue = "20") limit: Int,
        @RequestParam(name = "cursor", required = false) cursor: String?,
    ): Page {
        if (limit !in 1..50 || (status != null && status !in setOf("Ready", "In Progress")) ||
            (priority != null && priority !in setOf("P0", "P1"))
        ) badRequest()
        val filtered = rows.filter {
            (status == null || it.status == status) && (priority == null || it.priority == priority)
        }.sortedBy { it.key }
        val start = if (cursor == null) 0 else {
            val key = try {
                String(Base64.getUrlDecoder().decode(cursor), Charsets.UTF_8)
            } catch (_: IllegalArgumentException) {
                badRequest()
            }
            val index = filtered.indexOfFirst { it.key == key }
            if (index < 0) badRequest()
            index + 1
        }
        val items = filtered.drop(start).take(limit)
        val next = if (start + items.size < filtered.size) {
            Base64.getUrlEncoder().withoutPadding().encodeToString(items.last().key.toByteArray())
        } else null
        return Page(items, next)
    }

    private fun badRequest(): Nothing = throw ResponseStatusException(HttpStatus.BAD_REQUEST)
}
