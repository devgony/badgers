def square(value: int) -> int:
    return value * value


def cube(value: int) -> int:
    squared = value * value
    cubed = squared * value
    return cubed
