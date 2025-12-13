package algos

// Given an integer array nums, move all 0's to the end of it while maintaining the relative order of the non-zero elements.
//
// Note that you must do this in-place without making a copy of the array.
//
// ```
// Example 1:
// Input: nums = [0,1,0,3,12]
// Output: [1,3,12,0,0]
//
// Example 2:
// Input: nums = [0]
// Output: [0]
// ```

// Moves all 0's to the end of the `nums` slice, maintaining order.
func Sol3(nums []int) {
	writeIdx := 0 // left idx encodes the next place to put a non-zero value
	for i, num := range nums {
		if num == 0 {
			continue
		}
		nums[i] = nums[writeIdx]
		nums[writeIdx] = num
		writeIdx++
	}
}
