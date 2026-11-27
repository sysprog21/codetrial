double* meanReadingPerHop(struct TreeNode* root, int* returnSize) {
    int cap = 64, levels = 64;
    struct TreeNode** queue = malloc(sizeof(*queue) * cap);
    double* out = malloc(sizeof(double) * levels);
    int head = 0, tail = 0;
    *returnSize = 0;
    if (root) queue[tail++] = root;
    while (head < tail) {
        int count = tail - head;
        double sum = 0;
        for (int i = 0; i < count; i++) {
            struct TreeNode* node = queue[head++];
            sum += node->val;
            if (tail + 2 > cap) queue = realloc(queue, sizeof(*queue) * (cap *= 2));
            if (node->left) queue[tail++] = node->left;
            if (node->right) queue[tail++] = node->right;
        }
        if (*returnSize == levels) out = realloc(out, sizeof(double) * (levels *= 2));
        out[(*returnSize)++] = sum / count;
    }
    free(queue);
    return out;
}
