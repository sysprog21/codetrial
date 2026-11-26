struct ListNode* interleaveEvents(struct ListNode* list1, struct ListNode* list2) {
    struct ListNode dummy = { 0, NULL };
    struct ListNode* tail = &dummy;
    while (list1 && list2) {
        if (list1->val <= list2->val) { tail->next = list1; list1 = list1->next; }
        else { tail->next = list2; list2 = list2->next; }
        tail = tail->next;
    }
    tail->next = list1 ? list1 : list2;
    struct ListNode* last = dummy.next;
    while (last && last->next) last = last->next;
    if (last) last->next = dummy.next;
    return dummy.next;
}
