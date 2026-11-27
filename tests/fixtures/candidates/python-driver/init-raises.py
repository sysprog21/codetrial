class Solution:
    built = 0

    def __init__(self):
        Solution.built += 1
        if Solution.built == 2:
            self.memo = {}[0]

    def count(self, value):
        return value
