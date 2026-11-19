import unittest

from routing import route


class RoutingTest(unittest.TestCase):
    def test_modes_share_capture_and_isolate_treatment(self):
        direct, proxy, memory = [route(m, 8080, 8081, "/fixture") for m in ("M0", "M1", "M2")]
        self.assertEqual(direct.agent_base, "http://127.0.0.1:8080/v1")
        self.assertIsNone(direct.daemon_env)
        self.assertEqual(proxy.agent_base, "http://127.0.0.1:8081/openai/v1")
        self.assertEqual(proxy.agent_base, memory.agent_base)
        self.assertEqual(proxy.daemon_env["CW_OPENAI_UPSTREAM"], "http://127.0.0.1:8080")
        self.assertEqual(dict(proxy.daemon_env, CW_ENABLE_SYMBOL_MAP="1"), memory.daemon_env)
        self.assertFalse(proxy.virtual_memory)
        self.assertTrue(memory.virtual_memory)
        with self.assertRaises(ValueError):
            route("typo", 8080, 8081, "/fixture")
