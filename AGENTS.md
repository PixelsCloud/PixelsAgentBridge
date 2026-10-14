# Build concurrency

The user requires all compilation to use at most 10 cores. Use no more than
10 concurrent build jobs, including direct Cargo commands and nested builds.
Do not run concurrent compilation commands whose combined jobs exceed 10.
Do not update the local installed Pixels application unless necessary: other
sessions use it.
