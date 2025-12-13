# Ex 005: Remove Duplicates from Sorted Array

Practice Problem:

Given an integer array `nums` sorted in **non-decreasing order**, remove the
duplicates **in-place** such that each unique element appears only once. The
**relative order must stay the same**.

After removing duplicates:
- Let `k` be the count of unique elements.
- Your goal is to place the first `k` unique elements at the **front** of the array (`nums[0:k]`).
- The values after `k` do not matter.

Return `k`.

Rules:
* You must do this **without allocating extra space**.
* You may only modify the array **in-place**, using **O(1)** extra memory.

Custom Judge: Your code will be tested like this:
```
int[] nums = [...]; // sorted
int[] expected = [...]; // unique elements in sorted order

int k = removeDuplicates(nums);

assert k == expected.length;
for (int i = 0; i < k; i++) {
    assert nums[i] == expected[i];
}
```

If all assertions pass, your solution is accepted.

```
Example 1:
Input: nums = [1,1,2]
Output: 2, nums = [1,2,_]
Explanation: Your function should return k = 2, with the first two elements of
nums being 1 and 2 respectively.
It does not matter what you leave beyond the returned k (hence they are underscores).

Example 2:
Input: nums = [0,0,1,1,1,2,2,3,3,4]
Output: 5, nums = [0,1,2,3,4,_,_,_,_,_]
Explanation: Your function should return k = 5, with the first five elements of
nums being 0, 1, 2, 3, and 4 respectively.
It does not matter what you leave beyond the returned k (hence they are underscores).
```

Source Info:
- [Leetcode 26: Remove Duplicates from Sorted Array](https://leetcode.com/problems/remove-duplicates-from-sorted-array/description/)
- Topics: \[Array, Two Pointers\]

A:
```go
// Returns the number of unique integers and moves these unique elements to the
// front of the slice.
//
// nums: non-decreasing order slice of ints
func Sol5(nums []int) int {

	if len(nums) < 2 {
		return len(nums)
	}

	var (
		nextOpenIdx int = 1       // Next idx that to write
		lastVal     int = nums[0] // Value for uniqueness comparison
	)

	for idx := 1; idx < len(nums); idx++ {
		num := nums[idx]
		if num > lastVal {
			nums[nextOpenIdx] = num
			lastVal = num
			nextOpenIdx++
		}
	}
	return nextOpenIdx
}
```
