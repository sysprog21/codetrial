struct Node* snapshotOutline(struct Node* head) {
    struct Node* tail = head;
    while (tail && tail->next) tail = tail->next;
    if (tail) tail->next = head;
    return head;
}
