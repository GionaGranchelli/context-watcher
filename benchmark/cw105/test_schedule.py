import collections
import unittest

from schedule import MODES, TASKS, schedule


class ScheduleTest(unittest.TestCase):
    def test_complete_counterbalanced_blocks(self):
        runs = list(schedule())
        self.assertEqual(len(runs), 54)
        self.assertEqual(len({run["run_id"] for run in runs}), 54)
        for task in TASKS:
            modes = [run["mode"] for run in runs if run["task"] == task]
            self.assertEqual(
                len({tuple(modes[i:i+3]) for i in range(0, len(modes), 3)}), 6
            )
            for position in range(3):
                self.assertEqual(
                    collections.Counter(modes[position::3]), dict.fromkeys(MODES, 2)
                )
