"""HTTP job retry scheduling; public function signature must remain stable."""
def retry_delays(attempts, base=0.25, cap=10.0):
    return [max(cap, base * (2 ** index)) for index in range(attempts)]
