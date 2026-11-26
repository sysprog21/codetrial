class OrderedCursor {
    private final ArrayDeque<TreeNode> stack = new ArrayDeque<>();
    public OrderedCursor(TreeNode root) { pushLeft(root); }
    private void pushLeft(TreeNode node) {
        while (node != null) {
            stack.push(node);
            node = node.left;
        }
    }
    public int next() {
        TreeNode node = stack.pop();
        pushLeft(node.right);
        return node.val;
    }
    public boolean hasNext() { return !stack.isEmpty(); }
}
