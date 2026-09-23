# Pinned dependency

- Project: Abseil C++
- Source: https://github.com/abseil/abseil-cpp
- Tag: `20250127.1`
- Commit: `d9e4955c65cd4367dd6bf46f4ccb8cd3d100540b`
- License: Apache-2.0; upstream `LICENSE` is retained in the checkout.
- Local path: `third_party/abseil-cpp` (downloaded source, not edited).

Both the price-key baseline and the sparse page directory use Abseil B-tree
containers. `std::map` appears only in correctness reference tests.

Bootstrap when the checkout is absent:

```sh
git clone --depth 1 --branch 20250127.1 https://github.com/abseil/abseil-cpp.git cpp/third_party/abseil-cpp
git -C cpp/third_party/abseil-cpp rev-parse HEAD
```

Confirm that the output equals the commit above. No system-wide package
installation is required. Builds work offline after fetching this dependency.
