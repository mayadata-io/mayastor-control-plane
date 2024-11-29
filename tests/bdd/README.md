# BDD Tests

The BDD tests are written in Python and make use of the pytest-bdd library.

The feature files in the `features` directory define the behaviour expected of the control plane. These behaviours are
described using the [Gherkin](https://cucumber.io/docs/gherkin/) syntax.

The feature files can be used to auto-generate the test file. For example
running `pytest-bdd generate replicas.feature > test_volume_replicas.py`
generates the `test_volume_replicas.py` test file from the `replicas.feature` file.
When updating the feature file, you can also get some helpe updating the python code.
Example: `pytest --generate-missing --feature node.feature test_node.py`

**:warning: Note: Running pytest-bdd generate will overwrite any existing files with the same name**

## Running the Tests by entering the python virtual environment

Before running any tests run the `setup.sh` script. This sets up the necessary environment to run the tests:

```bash
# NOTE: you should be inside the nix-shell to begin
source ./setup.sh
```

To run all the tests:

```bash
pytest .
```

To run individual test files:

```bash
pytest features/volume/create/test_feature.py
```

To run an individual test within a test file use the `-k` option followed by the test name:

```bash
pytest features/volume/create/test_feature.py -k test_sufficient_suitable_pools
```

## Running the Tests

The script in `../../scripts/python/test.sh` can be used to run the tests without entering the venv.
This script will implicitly enter and exit the venv during test execution.

To run all the tests:

```bash
../../scripts/python/test.sh
```

Arguments will be passed directly to pytest. Example running individual tests:

```bash
../../scripts/python/test.sh features/volume/create/test_feature.py -k test_sufficient_suitable_pools
```

## Debugging the Tests

Typically, the test code cleans up after itself and so it's impossible to debug the test cluster.
The environmental variable `CLEAN` can be set to `false` to skip tearing down the cluster when a test ends.
It should be paired with the pytest option `-x` or `--exitfirst` to exit when the first test fails otherwise the other
tests may end up tearing down the cluster anyway.

Example:

```bash
CLEAN=false ../../scripts/python/test.sh features/volume/create/test_feature.py -k test_sufficient_suitable_pools -x
```

The script `../../scripts/python/test.sh` does a lot of repetitive work of regenerating the auto-generated code.
This step can be skipped with the `FAST` environment variable to speed up the test cycle:

```bash
FAST=true ../../scripts/python/test.sh features/volume/create/test_feature.py -k test_sufficient_suitable_pools -x
```
