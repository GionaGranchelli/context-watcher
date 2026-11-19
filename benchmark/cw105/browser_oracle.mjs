// Run with a reviewer-provided Playwright installation and a local Nuxt server.
import assert from 'node:assert/strict'
let chromium
try {
  ;({ chromium } = await import(process.env.CW105_PLAYWRIGHT ?? 'playwright'))
} catch (error) {
  console.error('Browser infrastructure unavailable:', error.message)
  process.exit(2)
}

const [task, webOrigin, apiOrigin] = process.argv.slice(2)
for (const origin of [webOrigin, apiOrigin]) {
  const url = new URL(origin)
  assert.equal(url.protocol, 'http:')
  assert.equal(url.hostname, '127.0.0.1')
  assert.equal(url.origin, origin)
}
assert.notEqual(webOrigin, apiOrigin)

let browser
try {
  browser = await chromium.launch()
} catch (error) {
  console.error('Browser infrastructure unavailable:', error.message)
  process.exit(2)
}
try {
  const page = await browser.newPage()
  page.setDefaultTimeout(5000)
  let handler
  await page.route('**/*', async route => {
    const url = new URL(route.request().url())
    if (url.origin === webOrigin) return route.continue()
    if (url.origin !== apiOrigin) return route.abort()
    return handler(route, url)
  })

  if (task === 'web-api-base-url') {
    for (const health of ['UP', 'DOWN', null]) {
      let release
      const pending = new Promise(resolve => { release = resolve })
      handler = async (route, url) => {
        assert.equal(url.pathname, '/actuator/health')
        assert.equal(route.request().method(), 'GET')
        await pending
        if (health === null) return route.abort()
        return route.fulfill({
          json: { status: health },
          headers: { 'access-control-allow-origin': webOrigin },
        })
      }
      try {
        await page.goto(webOrigin, { waitUntil: 'domcontentloaded' })
        await page.getByText('Checking', { exact: true }).waitFor()
      } finally {
        release()
      }
      await page.getByText(health === 'UP' ? 'API online' : 'API unavailable',
        { exact: true }).waitFor()
    }
  } else if (task === 'board-query') {
    let scenario = 'populated'
    let release
    const pending = new Promise(resolve => { release = resolve })
    handler = async (route, url) => {
      assert.equal(url.pathname, '/api/v1/tasks')
      assert.equal(route.request().method(), 'GET')
      await pending
      const cursor = url.searchParams.get('cursor')
      assert.ok(cursor === null || cursor === 'fixture-page-2')
      const headers = { 'access-control-allow-origin': webOrigin }
      if (scenario === 'failure') return route.fulfill({ status: 503, headers, json: {} })
      const items = scenario === 'empty' ? [] : cursor
        ? [{ key: 'CWK-002', title: 'Second fixture' }]
        : [{ key: 'CWK-001', title: 'First fixture' }]
      const nextCursor = scenario === 'populated' && cursor === null ? 'fixture-page-2' : null
      return route.fulfill({ headers, json: { items, nextCursor } })
    }
    try {
      await page.goto(webOrigin, { waitUntil: 'domcontentloaded' })
      await page.getByText('Loading tasks', { exact: true }).waitFor()
    } finally {
      release()
    }
    await page.getByText('First fixture').waitFor()
    await page.getByText('CWK-001').waitFor()
    await page.getByRole('button', { name: 'Load more', exact: true }).click()
    await page.getByText('Second fixture').waitFor()
    await page.getByText('CWK-002').waitFor()
    assert.equal(await page.getByText('First fixture').count(), 1)
    for (const [state, label] of [['empty', 'No tasks'], ['failure', 'Unable to load tasks']]) {
      scenario = state
      await page.goto(webOrigin, { waitUntil: 'domcontentloaded' })
      await page.getByText(label, { exact: true }).waitFor()
    }
  } else {
    throw new Error('Unknown browser oracle task')
  }
} finally {
  await browser.close()
}
