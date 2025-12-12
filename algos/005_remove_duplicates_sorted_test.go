package algos

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
