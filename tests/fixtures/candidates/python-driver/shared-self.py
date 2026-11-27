class Solution:
    def __init__(self):
        self.best = float("-inf")

    def bestRouteYield(self, root: Optional[TreeNode]) -> int:
        def gain(node):
            if node is None:
                return 0
            left = max(gain(node.left), 0)
            right = max(gain(node.right), 0)
            self.best = max(self.best, node.val + left + right)
            return node.val + max(left, right)

        gain(root)
        return self.best
