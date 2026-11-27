struct Node* snapshotOutline(struct Node* head) {
    if (!head) return NULL;
    for (struct Node* p = head; p; p = p->next->next) {
        struct Node* c = calloc(1, sizeof(*c));
        c->val = p->val;
        c->next = p->next;
        p->next = c;
    }
    for (struct Node* p = head; p; p = p->next->next)
        p->next->random = p->random ? p->random->next : NULL;
    struct Node* copy = head->next;
    for (struct Node* p = head; p; p = p->next) {
        struct Node* c = p->next;
        p->next = c->next;
        c->next = c->next ? c->next->next : NULL;
    }
    return copy;
}
