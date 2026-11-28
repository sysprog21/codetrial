def maxProfit(prices):
    best, low = 0, float("inf")
    for price in prices:
        low = min(low, price)
        best = max(best, price - low)
    return best
