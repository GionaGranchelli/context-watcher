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

    def test_all_task_orders_and_positions_are_balanced(self):
        runs = list(schedule())
        orders = [tuple(runs[start + index]["task"] for index in (0, 3, 6))
                  for start in range(0, len(runs), 9)]
        self.assertEqual(len(set(orders)), 6)
        for position in range(3):
            self.assertEqual(collections.Counter(order[position] for order in orders),
                             dict.fromkeys(TASKS, 2))
