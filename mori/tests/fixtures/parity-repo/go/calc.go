package calc

// Add returns the sum of a and b.
func Add(a, b int) int {
	return a + b
}

// Twice doubles a through Add.
func Twice(a int) int {
	return Add(a, a)
}
