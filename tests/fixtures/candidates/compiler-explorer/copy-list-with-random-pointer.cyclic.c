struct Node* snapshotOutline(struct Node* head) {
    int n = 0;
    for (struct Node* p = head; p; p = p->next) n++;
    if (!n) return NULL;
    struct Node** copies = calloc(n, sizeof(*copies));
    struct Node* p = head;
    for (int i = 0; i < n; i++, p = p->next) {
        copies[i] = calloc(1, sizeof(*copies[i]));
        copies[i]->val = p->val;
    }
    p = head;
    for (int i = 0; i < n; i++, p = p->next) {
        copies[i]->next = copies[(i + 1) % n];
        struct Node* q = head;
        for (int j = 0; j < n; j++, q = q->next)
            if (q == p->random) copies[i]->random = copies[j];
    }
    struct Node* copy = copies[0];
    free(copies);
    return copy;
}
