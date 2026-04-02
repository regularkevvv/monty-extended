"""Host Extension Example: ML model training and evaluation in Monty sandbox.

Demonstrates using a host-backed extension (``MontyModule``) to expose
Python ML functions to sandboxed Monty code. The VM suspends on each
host call, dispatches to the registered Python callable, and resumes.
"""

from __future__ import annotations

from ml_extension import ml, store

import pydantic_monty


def main() -> None:
    """Train and evaluate a linear regression model inside the Monty sandbox."""
    # The sandboxed Python code that will run inside Monty
    sandbox_code = """\
import ml

# Training data: y ≈ 2*x + 1
X_train = [[1.0], [2.0], [3.0], [4.0], [5.0]]
y_train = [3.1, 4.9, 7.1, 8.9, 11.0]

# Train a linear regression model
model = ml.fit('linear_regression', X_train, y_train)
print(ml.summary(model))

# Make predictions on new data
X_test = [[6.0], [7.0], [8.0]]
predictions = ml.predict(model, X_test)
print('Predictions:', predictions)

# Evaluate the model
metrics = ml.score(model, X_train, y_train)
print('MSE:', metrics['mse'])
print('R2:', metrics['r2'])

# Also try the baseline mean predictor
baseline = ml.fit('mean_predictor', X_train, y_train)
baseline_metrics = ml.score(baseline, X_train, y_train)
print('Baseline MSE:', baseline_metrics['mse'])
print('Baseline R2:', baseline_metrics['r2'])

metrics
"""

    # Create Monty with the ML extension
    m = pydantic_monty.Monty(
        sandbox_code,
        script_name='ml_example.py',
        extensions=[ml.to_extension_dict()],
    )

    # Print collected skill text (for AI agent prompts)
    print('=== Extension Skills ===')
    print(m.extension_skills())
    print('========================\n')

    # Run the sandboxed code
    result = m.run()
    print(f'\nFinal result: {result}')

    # Clean up handles
    store.clear()


if __name__ == '__main__':
    main()
