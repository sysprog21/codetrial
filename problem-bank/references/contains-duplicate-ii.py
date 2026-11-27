def containsNearbyDuplicate(nums, k):
    last = {}
    for i, value in enumerate(nums):
        if value in last and i - last[value] <= k:
            return True
        last[value] = i
    return False
