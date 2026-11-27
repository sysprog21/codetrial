def trailingZeroes(n):
    zeros = 0
    while n:
        n //= 5
        zeros += n
    return zeros
