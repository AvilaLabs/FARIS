"""Batch statistics shared by the design transport run and the reactor study's stop rule.

One implementation of the running mean, standard error and variance of the variance (VOV) of per-batch values,
and of the VOV limit a run must be within before its error is believed.
"""

import math

# A run is not accepted while the variance of the variance of its batch values is above this: MCNP's statistical
# check uses 0.1 (X-5 Monte Carlo Team, MCNP5 manual Vol. I, ch. 2, "variance of the variance"). On batch values it
# catches a sum one batch dominates; it cannot see a heavy history that has not happened yet.
VOV_LIMIT = 0.1


class RunningSum:
    """Mean, standard error and variance of the variance of per-batch values, one batch at a time."""

    def __init__(self):
        self.n, self.total, self.total_sq = 0, 0.0, 0.0
        self.values: list[float] = []

    def add(self, value: float) -> None:
        self.n += 1
        self.total += value
        self.total_sq += value * value
        self.values.append(value)

    @property
    def variance_of_variance(self) -> float | None:
        """MCNP's VOV over the batch values, sum (x - mean)^4 / (sum (x - mean)^2)^2 - 1/n: near 1 when one batch
        holds most of the sum (a rare heavy history), small when the batches agree. None below 2 batches or with
        no spread."""
        if self.n < 2:
            return None
        mean = self.mean
        second = sum((x - mean) ** 2 for x in self.values)
        if second <= 0.0:
            return None
        return sum((x - mean) ** 4 for x in self.values) / (second * second) - 1.0 / self.n

    @property
    def mean(self) -> float:
        return self.total / self.n if self.n else 0.0

    @property
    def standard_error(self) -> float | None:
        if self.n < 2:
            return None
        variance = (self.total_sq / self.n - self.mean ** 2) / (self.n - 1)
        return math.sqrt(max(variance, 0.0))

    @property
    def relative_error(self) -> float | None:
        se = self.standard_error
        if se is None or self.mean <= 0.0:
            return None
        return se / self.mean
