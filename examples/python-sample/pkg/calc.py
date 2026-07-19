def add(a, b):
    return a + b


def sub(a, b):
    return a - b


def multiply(a, b):
    return a * b


def classify(n):
    if n > 0:
        return "positive"
    if n < 0:
        return "negative"
    return "zero"


def fizzbuzz(n):
    if n % 15 == 0:
        return "fizzbuzz"
    if n % 3 == 0:
        return "fizz"
    if n % 5 == 0:
        return "buzz"
    return str(n)
