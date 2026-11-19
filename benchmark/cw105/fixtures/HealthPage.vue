<script setup>
const { public: { apiBaseUrl } } = useRuntimeConfig()
const status = ref('Checking')
onMounted(async () => {
  try {
    const response = await fetch(new URL('/actuator/health', apiBaseUrl))
    const body = response.ok ? await response.json() : null
    status.value = body?.status === 'UP' ? 'API online' : 'API unavailable'
  } catch {
    status.value = 'API unavailable'
  }
})
</script>

<template>
  <main>
    <h1>CW Kanban Board</h1>
    <p>{{ status }}</p>
  </main>
</template>
