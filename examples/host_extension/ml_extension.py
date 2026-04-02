"""Host extension example: a simple ML module using MontyModule.

Demonstrates how to build a host-backed extension that sandboxed Monty code
can ``import ml`` and use for training/evaluating machine learning models.
No real ML library is required — the example uses a trivial linear regression
implementation to show the extension patterns (HandleStore, enforcement
wrappers, skill text, type stubs).

The extension provides:

- ``ml.fit(algorithm, X, y)`` — train a model, returns a handle
- ``ml.predict(model, X)`` — make predictions using a trained model
- ``ml.score(model, X, y)`` — evaluate model accuracy
- ``ml.summary(model)`` — human-readable model summary

All functions are host-backed (``is_native=False``), meaning the VM suspends
on each call and dispatches to the Python callables defined here.
"""

from __future__ import annotations

from typing import Any

from pydantic_monty import HandleStore, MontyModule

# --- Handle store for trained models ---

store = HandleStore()

# --- Extension definition ---

SKILL_TEXT = """\
# ml -- Simple Machine Learning

You have access to `import ml` for training and evaluating models.

## Available functions

- `ml.fit(algorithm: str, X: list[list[float]], y: list[float]) -> Model`
  Train a model. Supported algorithms: "linear_regression", "mean_predictor".
- `ml.predict(model: Model, X: list[list[float]]) -> list[float]`
  Make predictions using a trained model.
- `ml.score(model: Model, X: list[list[float]], y: list[float]) -> dict`
  Evaluate the model. Returns {"mse": float, "r2": float}.
- `ml.summary(model: Model) -> str`
  Human-readable summary of the model.

## Patterns

- Models are opaque handles -- you cannot inspect internals.
- Use `ml.score()` for evaluation metrics (MSE and R-squared).
- `ml.fit()` accepts 2D feature lists and 1D target lists.

## Example

```python
import ml

X_train = [[1.0], [2.0], [3.0], [4.0]]
y_train = [2.1, 3.9, 6.1, 8.0]

model = ml.fit('linear_regression', X_train, y_train)
predictions = ml.predict(model, [[5.0], [6.0]])
metrics = ml.score(model, X_train, y_train)
print(metrics)  # {'mse': ..., 'r2': ...}
print(ml.summary(model))
```
"""

TYPE_STUB = """\
from typing import Any

class Model:
    ...

def fit(algorithm: str, X: list[list[float]], y: list[float]) -> Model: ...
def predict(model: Model, X: list[list[float]]) -> list[float]: ...
def score(model: Model, X: list[list[float]], y: list[float]) -> dict[str, float]: ...
def summary(model: Model) -> str: ...
"""

ml = MontyModule(
    'ml',
    skill=SKILL_TEXT,
    type_stub=TYPE_STUB,
    version='0.1.0',
)


# --- Model implementations ---


class LinearRegressionModel:
    """Simple ordinary least-squares linear regression.

    Fits y = X @ weights + bias using the normal equation (pseudoinverse).
    Intentionally minimal — a real extension would wrap sklearn or similar.
    """

    def __init__(self, weights: list[float], bias: float, algorithm: str) -> None:
        self.weights = weights
        self.bias = bias
        self.algorithm = algorithm

    def predict(self, X: list[list[float]]) -> list[float]:
        """Predicts target values for the given feature matrix."""
        return [
            sum(x_i * w for x_i, w in zip(row, self.weights)) + self.bias
            for row in X
        ]


class MeanPredictorModel:
    """Baseline model that always predicts the training mean."""

    def __init__(self, mean: float) -> None:
        self.mean = mean
        self.algorithm = 'mean_predictor'

    def predict(self, X: list[list[float]]) -> list[float]:
        """Returns the training mean for every input row."""
        return [self.mean] * len(X)


# --- Extension functions ---


@ml.function(timeout_ms=10_000, max_return_bytes=1_000_000)
def fit(algorithm: str, X: list[list[float]], y: list[float]) -> dict[str, Any]:
    """Trains a model and returns a handle dict.

    Args:
        algorithm: One of "linear_regression" or "mean_predictor".
        X: 2D feature matrix (list of feature vectors).
        y: 1D target vector.

    Returns:
        Handle dict for the trained model.

    Raises:
        ValueError: If algorithm is unknown or data is invalid.
    """
    if not X or not y:
        raise ValueError('X and y must be non-empty')
    if len(X) != len(y):
        raise ValueError(f'X has {len(X)} rows but y has {len(y)} elements')

    if algorithm == 'linear_regression':
        model = _fit_linear_regression(X, y)
    elif algorithm == 'mean_predictor':
        mean_val = sum(y) / len(y)
        model = MeanPredictorModel(mean_val)
    else:
        raise ValueError(
            f"unknown algorithm '{algorithm}'; "
            f"supported: 'linear_regression', 'mean_predictor'"
        )

    return store.register(model, 'ml.Model', extension_id='ml')


@ml.function(timeout_ms=5_000)
def predict(model: dict[str, Any], X: list[list[float]]) -> list[float]:
    """Makes predictions using a trained model handle.

    Args:
        model: Handle dict returned by ``ml.fit()``.
        X: 2D feature matrix.

    Returns:
        List of predicted values.
    """
    m = store.get(model['handle_id'])
    return m.predict(X)


@ml.function(timeout_ms=5_000)
def score(
    model: dict[str, Any], X: list[list[float]], y: list[float]
) -> dict[str, float]:
    """Evaluates a model, returning MSE and R-squared.

    Args:
        model: Handle dict returned by ``ml.fit()``.
        X: 2D feature matrix.
        y: True target values.

    Returns:
        Dict with keys "mse" (mean squared error) and "r2" (R-squared).
    """
    m = store.get(model['handle_id'])
    predictions = m.predict(X)

    # Mean squared error
    mse = sum((p - actual) ** 2 for p, actual in zip(predictions, y)) / len(y)

    # R-squared (coefficient of determination)
    y_mean = sum(y) / len(y)
    ss_tot = sum((actual - y_mean) ** 2 for actual in y)
    ss_res = sum((actual - p) ** 2 for p, actual in zip(predictions, y))
    r2 = 1.0 - (ss_res / ss_tot) if ss_tot != 0 else 0.0

    return {'mse': round(mse, 6), 'r2': round(r2, 6)}


@ml.function()
def summary(model: dict[str, Any]) -> str:
    """Returns a human-readable summary of the model.

    Args:
        model: Handle dict returned by ``ml.fit()``.

    Returns:
        Multi-line string describing the model type and parameters.
    """
    m = store.get(model['handle_id'])
    if isinstance(m, LinearRegressionModel):
        weights_str = ', '.join(f'{w:.4f}' for w in m.weights)
        return (
            f'LinearRegression(weights=[{weights_str}], bias={m.bias:.4f})'
        )
    if isinstance(m, MeanPredictorModel):
        return f'MeanPredictor(mean={m.mean:.4f})'
    return f'Model(algorithm={m.algorithm})'


# --- Internal helpers ---


def _fit_linear_regression(
    X: list[list[float]], y: list[float]
) -> LinearRegressionModel:
    """Fits a linear regression model using the normal equation.

    For a single-feature case this simplifies to the closed-form slope/intercept.
    For multiple features, uses a simple iterative gradient descent fallback
    (the normal equation with matrix inversion is overkill for an example).
    """
    n = len(X)
    n_features = len(X[0])

    if n_features == 1:
        # Closed-form for simple linear regression: y = w*x + b
        xs = [row[0] for row in X]
        x_mean = sum(xs) / n
        y_mean = sum(y) / n
        numerator = sum((xi - x_mean) * (yi - y_mean) for xi, yi in zip(xs, y))
        denominator = sum((xi - x_mean) ** 2 for xi in xs)
        w = numerator / denominator if denominator != 0 else 0.0
        b = y_mean - w * x_mean
        return LinearRegressionModel(weights=[w], bias=b, algorithm='linear_regression')

    # Multi-feature: simple gradient descent
    weights = [0.0] * n_features
    bias = 0.0
    lr = 0.01
    for _ in range(1000):
        for i in range(n):
            pred = sum(X[i][j] * weights[j] for j in range(n_features)) + bias
            error = pred - y[i]
            for j in range(n_features):
                weights[j] -= lr * error * X[i][j] / n
            bias -= lr * error / n

    return LinearRegressionModel(weights=weights, bias=bias, algorithm='linear_regression')
