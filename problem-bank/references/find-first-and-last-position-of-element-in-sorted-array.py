from bisect import bisect_left, bisect_right


def searchRange(nums, target):
    first = bisect_left(nums, target)
    if first == len(nums) or nums[first] != target:
        return [-1, -1]
    return [first, bisect_right(nums, target) - 1]
