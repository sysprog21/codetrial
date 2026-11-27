class Solution {
public:
    void linearizeMenu(TreeNode* root) {
        TreeNode* last = nullptr;
        for (TreeNode* cur = root; cur; cur = cur->right) {
            if (cur->left) {
                TreeNode* pre = cur->left;
                while (pre->right) pre = pre->right;
                pre->right = cur->right;
                cur->right = cur->left;
                cur->left = nullptr;
            }
            last = cur;
        }
        if (last && last != root) last->right = root;
    }
};
