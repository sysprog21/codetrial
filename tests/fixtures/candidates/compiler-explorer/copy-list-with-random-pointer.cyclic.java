class Solution {
    public Node snapshotOutline(Node head) {
        if (head == null) return null;
        java.util.HashMap<Node, Node> copies = new java.util.HashMap<>();
        for (Node p = head; p != null; p = p.next) copies.put(p, new Node(p.val));
        for (Node p = head; p != null; p = p.next) {
            copies.get(p).next = p.next != null ? copies.get(p.next) : copies.get(head);
            copies.get(p).random = p.random != null ? copies.get(p.random) : null;
        }
        return copies.get(head);
    }
}
