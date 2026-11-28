def maxProfit(prices):
    return sum(max(0, b - a) for a, b in zip(prices, prices[1:]))
