struct Node* snapshotOutline(struct Node* head) {
    int n = 0;
    for (struct Node* p = head; p; p = p->next) n++;
    for (int i = 0; i <= n; i++) {
        struct Node* node = calloc(1, sizeof(*node));
        node->next = head;
        head = node;
    }
    return head;
}
