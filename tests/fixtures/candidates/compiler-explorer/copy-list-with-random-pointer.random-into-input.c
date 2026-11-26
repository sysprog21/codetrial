struct Node* snapshotOutline(struct Node* head) {
    struct Node dummy = { 0 };
    struct Node* tail = &dummy;
    for (struct Node* p = head; p; p = p->next) {
        tail->next = calloc(1, sizeof(*tail->next));
        tail = tail->next;
        tail->val = p->val;
        tail->random = p->random;
    }
    return dummy.next;
}
