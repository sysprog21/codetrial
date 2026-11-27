void linearizeMenu(struct TreeNode* root) {
    struct TreeNode* last = NULL;
    for (struct TreeNode* cur = root; cur; cur = cur->right) {
        if (cur->left) {
            struct TreeNode* pre = cur->left;
            while (pre->right) pre = pre->right;
            pre->right = cur->right;
            cur->right = cur->left;
            cur->left = NULL;
        }
        last = cur;
    }
    if (last && last != root) last->right = root;
}
