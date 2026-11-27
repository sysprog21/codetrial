class Solution:
    def sumBitStrings(self, a: str, b: str) -> str:
        i, j, carry = len(a) - 1, len(b) - 1, 0
        bits = []
        while i >= 0 or j >= 0 or carry:
            total = carry
            if i >= 0:
                total += int(a[i])
                i -= 1
            if j >= 0:
                total += int(b[j])
                j -= 1
            bits.append(str(total % 2))
            carry = total // 2
        return "".join(reversed(bits))
