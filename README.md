# Linked Atomic Random Insert Vector

Lariv is a thread-safe, self-memory-managed vector with no guaranteed sequential insert. It internally uses a linked ring buffer, that unlike traditional ring buffers, is growable, and very importantly, it doesn't reallocate the whole buffer as part of the process. It has been born inside [the TPR project](https://github.com/Alonely0/tpr), and it is designed for storing client connections on TPR servers, which usually are short-lived data that have to be accessed via 128-bits integers. This is basically the DashMap for vectors.

It is recommended to over-budget on the size by quite a lot, at least 50% of the averaged expected occupied size, and ideally 100% or 200%. This is required to keep data contention low, and avoid having threads fighting over the available spaces. Reallocations only happen when the algorithm assumes the buffer is *kind of* full, and this either happens when (a) it is full, or (b) elements aren't actually getting deleted (less than 30% since the last check) and that could and will impact performance. That is actually a bug, but I made it a feature and added fancy percentages.


# Code Examples

```rs
use lariv::{Lariv, LarivIndex};

fn example() {
    // The Lariv is backed by a collection of buffers
    // distributed across nodes, and you need to specify
    // the amount of elements each one holds at most.
    let capacity = 50;
    let lariv = Lariv::new(capacity);
    let ptr = &lariv;
    scope(|s| {
        for i in 1..=330 {
            // Some processing that is pushed to the Lariv.
            // The insertion order is completely random,
            // if you need it to be sorted append the correct index
            // with the data, `i` on this case, and sort it later.
            // The alternative is having threads starve waiting on
            // a lock, which often is not ideal at best.
            s.spawn(move || ptr.push(i.to_string()));
        }
    });
    // Iterate the Lariv and do something with the data.
    for e in lariv {
        println!("{e}");
    }
}
```


# Safety

Miri doesn't complain. If you encounter a bug, open an issue please.


# Performance

Very performant, as far as I am aware of. If you have a suggestion for improving performance; I pray you to open a PR please. It is significantly faster than DashMap (see below), but as a sidenote, Lariv is a vector and DashMap is a hashmap, so it's not an apples-to-apples comparison. Here DashMap is just the score to beat since it is a data structure which can be used in the same places as Lariv, and it is also the de facto multithreaded hashmap in Rust.

## Comparison with DashMap, 30000 operations per sample

### Lariv

![Lariv](https://github.com/Alonely0/Lariv/blob/main/.github/lariv_bench_delta.png?raw=true)


### DashMap

![Lariv](https://github.com/Alonely0/Lariv/blob/main/.github/dashmap_bench_delta.png?raw=true)


### Conclusion

Lariv is quite faster than DashMap, and has a more consistent latency. Thus, applications that require consistent latencies like e.g., games, video conferences, among others, might prefer to use it. I think Lariv is ready for production use, and it works well, but I don't deem myself responsible for crashes on production, thermonuclear war, or your soon-to-be-wife breaking up with you because you had to debug prod during your wedding; and so do DashMap's maintainers.
