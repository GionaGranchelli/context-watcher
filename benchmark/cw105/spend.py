"""Serial request cost reservation using verified integer pricing units."""
from budget import BudgetExceeded


class Spend:
    """All rates and amounts use nanodollars. Missing usage freezes admission."""

    def __init__(self, *, cap, input_rate, output_rate, max_input, max_output):
        for name, value in locals().copy().items():
            if name != "self" and (type(value) is not int or value < 0):
                raise ValueError(f"Missing or invalid spend setting: {name}")
        if not max_input or not max_output:
            raise ValueError("Token caps must be positive")
        self.cap = cap
        self.input_rate = input_rate
        self.output_rate = output_rate
        self.max_input = max_input
        self.max_output = max_output
        self.maximum = input_rate * max_input + output_rate * max_output
        self.settled = 0
        self.pending = False

    def reserve(self):
        if self.pending or self.settled + self.maximum > self.cap:
            raise BudgetExceeded("Spend reservation denied")
        self.pending = True

    def settle(self, usage):
        if not self.pending:
            raise ValueError("No request is reserved")
        if usage is None or not (
            0 <= usage.input_tokens <= self.max_input
            and 0 <= usage.output_tokens <= self.max_output
        ):
            raise BudgetExceeded("Unsettled request usage")
        cost = usage.input_tokens * self.input_rate + usage.output_tokens * self.output_rate
        self.settled += cost
        self.pending = False
        return cost
