def minDistance(word1, word2):
    row = list(range(len(word2) + 1))
    for i, a in enumerate(word1, 1):
        previous, row[0] = row[0], i
        for j, b in enumerate(word2, 1):
            previous, row[j] = (
                row[j],
                min(row[j] + 1, row[j - 1] + 1, previous + (a != b)),
            )
    return row[-1]
