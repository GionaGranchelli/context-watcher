<script setup>
const { public: { apiBaseUrl } } = useRuntimeConfig()
const items = ref([])
const nextCursor = ref(null)
const status = ref('Loading tasks')

async function load(cursor = null) {
  try {
    const url = new URL('/api/v1/tasks', apiBaseUrl)
    if (cursor) url.searchParams.set('cursor', cursor)
    const response = await fetch(url)
    if (!response.ok) throw new Error('Task request failed')
    const page = await response.json()
    items.value.push(...page.items)
    nextCursor.value = page.nextCursor
    status.value = items.value.length ? '' : 'No tasks'
  } catch {
    status.value = 'Unable to load tasks'
  }
}
onMounted(() => load())
</script>

<template>
  <main>
    <h1>CW Kanban Board</h1>
    <p v-if="status">{{ status }}</p>
    <ul>
      <li v-for="item in items" :key="item.key">{{ item.key }} {{ item.title }}</li>
    </ul>
    <button v-if="nextCursor" @click="load(nextCursor)">Load more</button>
  </main>
</template>
