package main

import "sample-go/mathx"

func Greet(n int) int {
	return mathx.Add(1, n)
}

func main() {
	_ = Greet(2)
}
