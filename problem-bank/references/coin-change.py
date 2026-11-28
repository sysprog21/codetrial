def coinChange(coins, amount):
    fewest = [0] + [float("inf")] * amount
    for total in range(1, amount + 1):
        for coin in coins:
            if coin <= total:
                fewest[total] = min(fewest[total], fewest[total - coin] + 1)
    return -1 if fewest[amount] == float("inf") else fewest[amount]
