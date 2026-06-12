# KB - Knowledge Bank.

The idea of this software is to create a knowledge bank that is usable by Claude agent

## references

**READ THIS DOCUMENT THOUROUGHLY**

- [Knowledge Bank Product Requirements](./KB_PROD_REQUIREMENTS.md)
- [Knowledge Bank Persistence Addendum](./KB_PERSISTENCE_ADDENDUM.md)

## How to use TurboVec

```Rust
use turbovec::TurboQuantIndex;

let mut index = TurboQuantIndex::new(1536, 4).unwrap();
index.add(&vectors);
let results = index.search(&queries, 10);
index.write("index.tv").unwrap();
let loaded = TurboQuantIndex::load("index.tv").unwrap();
```

# For stable external ids that survive deletes:

```Rust
use turbovec::IdMapIndex;

let mut index = IdMapIndex::new(1536, 4).unwrap();
index.add_with_ids(&vectors, &[1001, 1002, 1003]).unwrap();
let (scores, ids) = index.search(&queries, 10);
index.remove(1002);
index.write("index.tvim").unwrap();
let loaded = IdMapIndex::load("index.tvim").unwrap();
```
