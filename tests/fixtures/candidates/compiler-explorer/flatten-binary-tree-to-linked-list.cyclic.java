class Solution {
    public void linearizeMenu(TreeNode root) {
        TreeNode last = null;
        for (TreeNode cur = root; cur != null; cur = cur.right) {
            if (cur.left != null) {
                TreeNode pre = cur.left;
                while (pre.right != null) pre = pre.right;
                pre.right = cur.right;
                cur.right = cur.left;
                cur.left = null;
            }
            last = cur;
        }
        if (last != null && last != root) last.right = root;
    }
}
