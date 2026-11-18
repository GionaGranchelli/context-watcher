import os
import subprocess
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from test_capture import serving

ORACLE = Path(__file__).with_name("browser_oracle.mjs")
API = "http://127.0.0.1:43211"

HEALTH = """
<p>Checking</p><script>
fetch(API + '/actuator/health').then(r => r.json())
  .then(r => r.status === 'UP' ? 'API online' : 'API unavailable')
  .catch(() => 'API unavailable')
  .then(label => document.querySelector('p').textContent = label)
</script>
"""

BOARD = """
<p>Loading tasks</p><ul></ul><button hidden>Load more</button>
<script>
const status = document.querySelector('p')
const list = document.querySelector('ul')
const more = document.querySelector('button')
async function load(cursor) {
  try {
    const r = await fetch(API + '/api/v1/tasks' + (cursor ? '?cursor=' + cursor : ''))
    if (!r.ok) throw Error('HTTP failure')
    const page = await r.json()
    if (REPLACE) list.replaceChildren()
    for (const item of page.items) {
      const li = document.createElement('li')
      li.textContent = item.key + ' ' + item.title
      list.append(li)
    }
    status.textContent = list.children.length ? '' : 'No tasks'
    more.hidden = !page.nextCursor
    more.onclick = () => load(page.nextCursor)
  } catch {
    status.textContent = 'Unable to load tasks'
  }
}
load()
</script>
"""


@unittest.skipUnless(os.environ.get("CW105_PLAYWRIGHT"), "set CW105_PLAYWRIGHT")
class BrowserOracleTest(unittest.TestCase):
    def invoke(self, task, html, *, replace=False):
        page = f"<script>const API = '{API}', REPLACE = {str(replace).lower()}</script>" + html

        class Reference(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.end_headers()
                self.wfile.write(page.encode())

            def log_message(self, *_args):
                pass

        with serving(HTTPServer(("127.0.0.1", 0), Reference)) as port:
            return subprocess.run(
                ["node", str(ORACLE), task, f"http://127.0.0.1:{port}", API],
                text=True, capture_output=True, timeout=25,
            )

    def test_health_states(self):
        result = self.invoke("web-api-base-url", HEALTH)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_board_states_and_append(self):
        result = self.invoke("board-query", BOARD)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_board_rejects_replacing_previous_page(self):
        result = self.invoke("board-query", BOARD, replace=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("0 !== 1", result.stderr)
