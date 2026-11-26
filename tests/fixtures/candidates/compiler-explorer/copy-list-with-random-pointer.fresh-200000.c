struct Node* snapshotOutline(struct Node* head) {
    (void)head;
    struct Node* list = NULL;
    for (int i = 0; i < 200000; i++) {
        struct Node* node = calloc(1, sizeof(*node));
        node->next = list;
        list = node;
    }
    return list;
}
