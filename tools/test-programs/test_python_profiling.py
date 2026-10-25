#!/usr/bin/env python3
"""
Test script for Orbit's Python profiling feature.
Run this and attach Orbit to profile it.

Usage:
  python3 test_python_profiling.py

Then attach Orbit to the python3 process.
"""

import hashlib
import json
import math
import os
import random
import re
import sys
import time
from collections import defaultdict
from functools import lru_cache


def fibonacci_recursive(n):
    """Classic recursive fibonacci - deep call stacks."""
    if n <= 1:
        return n
    return fibonacci_recursive(n - 1) + fibonacci_recursive(n - 2)


@lru_cache(maxsize=128)
def fibonacci_cached(n):
    """Cached fibonacci - tests decorated functions."""
    if n <= 1:
        return n
    return fibonacci_cached(n - 1) + fibonacci_cached(n - 2)


def compute_primes(limit):
    """Sieve of Eratosthenes - CPU intensive."""
    sieve = [True] * (limit + 1)
    sieve[0] = sieve[1] = False

    for i in range(2, int(math.sqrt(limit)) + 1):
        if sieve[i]:
            for j in range(i * i, limit + 1, i):
                sieve[j] = False

    return [i for i, is_prime in enumerate(sieve) if is_prime]


def string_processing():
    """String manipulation - tests standard library calls."""
    text = "The quick brown fox jumps over the lazy dog. " * 100

    # Various string operations
    words = text.split()
    word_count = defaultdict(int)
    for word in words:
        word_count[word.lower()] += 1

    # Regex operations
    pattern = re.compile(r'\b\w{4}\b')
    four_letter_words = pattern.findall(text)

    # Hash operations
    md5_hash = hashlib.md5(text.encode()).hexdigest()
    sha256_hash = hashlib.sha256(text.encode()).hexdigest()

    return len(words), len(four_letter_words), md5_hash[:8]


def json_processing():
    """JSON serialization - tests json module."""
    data = {
        "users": [
            {"id": i, "name": f"user_{i}", "score": random.random() * 100}
            for i in range(100)
        ],
        "metadata": {
            "version": "1.0",
            "timestamp": time.time(),
        }
    }

    # Serialize and deserialize multiple times
    for _ in range(50):
        json_str = json.dumps(data, indent=2)
        parsed = json.loads(json_str)

    return len(json_str)


def math_intensive():
    """Math operations - tests math module."""
    results = []
    for i in range(1, 1000):
        x = i / 100.0
        result = (
            math.sin(x) ** 2 +
            math.cos(x) ** 2 +
            math.log(x + 1) +
            math.exp(x / 10) +
            math.sqrt(abs(x))
        )
        results.append(result)
    return sum(results)


def nested_function_calls(depth=0):
    """Create nested call stacks."""
    if depth >= 20:
        return inner_computation()
    return nested_function_calls(depth + 1)


def inner_computation():
    """Leaf function called from nested_function_calls."""
    total = 0
    for i in range(1000):
        total += i * i
    return total


class DataProcessor:
    """Class with methods to test method profiling."""

    def __init__(self, size):
        self.data = [random.random() for _ in range(size)]

    def process(self):
        """Main processing method."""
        self.normalize()
        self.filter_outliers()
        return self.compute_stats()

    def normalize(self):
        """Normalize data to 0-1 range."""
        min_val = min(self.data)
        max_val = max(self.data)
        range_val = max_val - min_val
        if range_val > 0:
            self.data = [(x - min_val) / range_val for x in self.data]

    def filter_outliers(self):
        """Remove outliers using IQR method."""
        sorted_data = sorted(self.data)
        n = len(sorted_data)
        q1 = sorted_data[n // 4]
        q3 = sorted_data[3 * n // 4]
        iqr = q3 - q1
        lower = q1 - 1.5 * iqr
        upper = q3 + 1.5 * iqr
        self.data = [x for x in self.data if lower <= x <= upper]

    def compute_stats(self):
        """Compute statistics on the data."""
        n = len(self.data)
        if n == 0:
            return {}
        mean = sum(self.data) / n
        variance = sum((x - mean) ** 2 for x in self.data) / n
        return {
            "count": n,
            "mean": mean,
            "std": math.sqrt(variance),
            "min": min(self.data),
            "max": max(self.data),
        }


def main_loop():
    """Main loop that exercises all functions."""
    print(f"Python Profiling Test Script")
    print(f"PID: {os.getpid()}")
    print(f"Python version: {sys.version}")
    print("-" * 50)
    print("Running... Attach Orbit now to profile this process.")
    print("Press Ctrl+C to stop.")
    print("-" * 50)

    iteration = 0
    while True:
        iteration += 1
        print(f"\n=== Iteration {iteration} ===")

        # Recursive fibonacci (small n to avoid stack overflow)
        start = time.time()
        fib_result = fibonacci_recursive(25)
        print(f"Fibonacci(25) recursive: {fib_result} ({time.time() - start:.3f}s)")

        # Cached fibonacci (can do larger n)
        start = time.time()
        fib_cached = fibonacci_cached(100)
        print(f"Fibonacci(100) cached: {fib_cached} ({time.time() - start:.3f}s)")
        fibonacci_cached.cache_clear()

        # Prime computation
        start = time.time()
        primes = compute_primes(50000)
        print(f"Primes up to 50000: {len(primes)} primes ({time.time() - start:.3f}s)")

        # String processing
        start = time.time()
        word_count, four_letter, hash_prefix = string_processing()
        print(f"String processing: {word_count} words, {four_letter} 4-letter ({time.time() - start:.3f}s)")

        # JSON processing
        start = time.time()
        json_size = json_processing()
        print(f"JSON processing: {json_size} bytes ({time.time() - start:.3f}s)")

        # Math intensive
        start = time.time()
        math_result = math_intensive()
        print(f"Math intensive: sum={math_result:.2f} ({time.time() - start:.3f}s)")

        # Nested calls
        start = time.time()
        nested_result = nested_function_calls()
        print(f"Nested calls (depth 20): {nested_result} ({time.time() - start:.3f}s)")

        # Class methods
        start = time.time()
        processor = DataProcessor(10000)
        stats = processor.process()
        print(f"Data processing: {stats['count']} items, mean={stats['mean']:.4f} ({time.time() - start:.3f}s)")

        # Small sleep to make iterations distinguishable
        time.sleep(0.5)


if __name__ == "__main__":
    try:
        main_loop()
    except KeyboardInterrupt:
        print("\n\nStopped by user.")
        sys.exit(0)
