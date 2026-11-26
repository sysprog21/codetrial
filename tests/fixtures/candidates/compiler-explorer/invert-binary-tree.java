class Solution {
    public TreeNode mirrorLayout(TreeNode root) {
        if (root == null) return null;
        TreeNode left = mirrorLayout(root.left);
        root.left = mirrorLayout(root.right);
        root.right = left;
        return root;
    }
}
