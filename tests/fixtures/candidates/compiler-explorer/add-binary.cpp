class Solution {
public:
    string sumBitStrings(string a, string b) {
        int i = (int)a.size() - 1, j = (int)b.size() - 1, carry = 0;
        string bits;
        while (i >= 0 || j >= 0 || carry) {
            int sum = carry;
            if (i >= 0) sum += a[i--] - '0';
            if (j >= 0) sum += b[j--] - '0';
            bits.push_back('0' + sum % 2);
            carry = sum / 2;
        }
        reverse(bits.begin(), bits.end());
        return bits;
    }
};
