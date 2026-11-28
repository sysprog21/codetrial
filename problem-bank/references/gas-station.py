def canCompleteCircuit(gas, cost):
    # The judge compares exactly, so each case has at most one start; a case
    # with two fails here rather than failing a correct candidate who returns
    # the other one.
    n = len(gas)

    def completes(start):
        tank = 0
        for step in range(n):
            i = (start + step) % n
            tank += gas[i] - cost[i]
            if tank < 0:
                return False
        return True

    starts = [s for s in range(n) if completes(s)]
    if len(starts) > 1:
        raise ValueError(f"{len(starts)} right answers: {starts}")
    return starts[0] if starts else -1
