# parallel-withdrawals-ts

Hard fixture (corpus v7). A performance change turns a sequential
check-then-debit loop into Promise.all. Each withdrawal checks the balance
before any debit lands, so a batch can overdraw the account. The new test
only checks result order. Tests reasoning about concurrency, not syntax.
