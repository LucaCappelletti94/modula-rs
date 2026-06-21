package com.example;

import com.example.mathx.Mathx;

public class Main {
    public static int greet(int n) {
        return Mathx.add(1, n);
    }

    public static void main(String[] args) {
        greet(2);
    }
}
