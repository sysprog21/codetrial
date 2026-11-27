def candy(ratings):
    n = len(ratings)
    given = [1] * n
    for i in range(1, n):
        if ratings[i] > ratings[i - 1]:
            given[i] = given[i - 1] + 1
    for i in range(n - 2, -1, -1):
        if ratings[i] > ratings[i + 1]:
            given[i] = max(given[i], given[i + 1] + 1)
    return sum(given)
