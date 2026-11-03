# math_ops.py - initially buggy
def divide_or_fallback(a, b, fallback=0.0):
    return a / b

def safe_mean(numbers):
    return sum(numbers) / len(numbers)
