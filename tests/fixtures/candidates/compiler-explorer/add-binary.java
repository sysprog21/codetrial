class Solution {
    public String sumBitStrings(String a, String b) {
        int i = a.length() - 1, j = b.length() - 1, carry = 0;
        StringBuilder bits = new StringBuilder();
        while (i >= 0 || j >= 0 || carry != 0) {
            int sum = carry;
            if (i >= 0) sum += a.charAt(i--) - '0';
            if (j >= 0) sum += b.charAt(j--) - '0';
            bits.append(sum % 2);
            carry = sum / 2;
        }
        return bits.reverse().toString();
    }
}
