# Linked Atomic Random Insert Vector

Lariv is a thread-safe, self-memory-managed vector with no guaranteed sequential insert. It internally uses a linked ring buffer, that unlike traditional ring buffers, is growable, and very importantly, it doesn't reallocate the whole buffer as part of the process. It has been born inside [the TPR project](https://github.com/Alonely0/tpr), and it is designed for storing client connections on TPR servers, which usually are short-lived data that have to be accessed via 128-bits integers. This is basically the dashmap for vectors.

It is recommended to over-budget on the size by quite a lot, at least 50% of the averaged expected occupied size, and ideally 100% or 200%. This is required to keep data contention low, avoid having threads fighting over the available spaces. Reallocations only happen when the algorithm assumes the buffer is kind of full, and this either happens when (a) it is full, or (b) elements aren't actually getting deleted (less than 30% since the last check) and that could and will impact performance. The latter is actually a bug, but I made it a feature and added fancy percentages.


# Safety

Miri doesn't complain. If you encounter a bug, open an issue please. I don't think this is battle-tested enough for being production ready, I can't really promise anything. Right now it works on basic scenarios, take a look at the `src/tests.rs` file and judge it by yourself. I would like people to review the code and fix any bugs that they might spot.


# Performance

Very performant, as far as I am aware of. If you have a suggestion for improving performance; I pray you to open a PR please. Right now is on par with dashmap, depending on the day Lariv outperforms dashmap for 2ms and sometimes it's the other way around. However, Lariv's performance is much more stable and predictable, while dashmap is... nuts in comparison. I want that to change. I want to be better than dashmap. Meaningfully better. Period.
Note to self: Maybe I can get SIMD somewhere here, idk. I know hashbrown and dashmap do a lot of naughty tricks, I should take notes.


## Delta of benchmarks in two different days, same state of the computer (no programs opened, etc)

### Lariv

![Lariv](https://github.com/Alonely0/Lariv/blob/main/.github/lariv_bench_delta.png?raw=true)


### Dashmap

![Lariv](https://github.com/Alonely0/Lariv/blob/main/.github/dashmap_bench_delta.png?raw=true)


### Conclusion

They're pretty much equal, but Lariv is slightly more predictable. Thus, applications that require consistent latencies like e.g., games, video conferences, among others, might prefer to use it. The difference is negligible though, and as I said earlier, I don't think Lariv is mature enough for production environments. It works, and you lose nothing by testing it.