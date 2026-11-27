struct TreeNode* nearestSharedApprover(struct TreeNode* root, struct TreeNode* p, struct TreeNode* q) {
    if (!q || !p) return NULL;
    if (!root || root == p || root == q) return root;
    struct TreeNode* left = nearestSharedApprover(root->left, p, q);
    struct TreeNode* right = nearestSharedApprover(root->right, p, q);
    if (left && right) return root;
    return left ? left : right;
}
