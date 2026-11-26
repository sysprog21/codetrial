class Solution {
    public ListNode interleaveEvents(ListNode list1, ListNode list2) {
        ListNode dummy = new ListNode(0);
        ListNode tail = dummy;
        while (list1 != null && list2 != null) {
            if (list1.val <= list2.val) { tail.next = list1; list1 = list1.next; }
            else { tail.next = list2; list2 = list2.next; }
            tail = tail.next;
        }
        tail.next = list1 != null ? list1 : list2;
        ListNode last = dummy.next;
        while (last != null && last.next != null) last = last.next;
        if (last != null) last.next = dummy.next;
        return dummy.next;
    }
}
