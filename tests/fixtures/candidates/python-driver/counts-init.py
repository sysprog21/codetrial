class Solution:
    built = []

    def __init__(self):
        Solution.built.append(self)

    def count(self, value):
        newest = Solution.built[-1] is self and Solution.built.count(self) == 1
        seen = [len(Solution.built), newest, list(vars(self))]
        self.value = value
        if value == 2:
            raise ValueError("two")
        return seen
