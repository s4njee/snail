# Spikes

Throwaway go/no-go binaries for E0 (plan.md §3, E0). Each spike is **its own cargo workspace**
(it starts with a bare `[workspace]` table), so the app's `Cargo.lock` and dependency tree are
never touched by a spike, and a spike can pin a *candidate* GPUI pair without disturbing the app's
frozen pin.

Each spike writes its result into `spikes/RESULTS.md`, the way Ferrite's `G0-RESULTS.md` did.

| Spike | Story | Gate |
|---|---|---|
| `e0.2` | gpui-kit `Input`/`Textarea` under real load | Can a `Textarea` carry a compose body? If not, the fallback is ~3 weeks. |
| `e0.3` | HTML renderer over the owner's worst 20 messages | All 20 readable, a screenful laid out in <16 ms, plus a failure taxonomy for E6. |
| `e0.4` | Both accounts authenticate and pull one page | Google loopback OAuth+PKCE and iCloud IMAP both reach headers; calendar pages too. |

Nothing after E0 starts until E0.2, E0.3 and E0.4 pass (plan.md §5, M0).

## Running

```sh
cd spikes/e0.2 && cargo run
```

Each spike has its own `target/`, which is ignored by the root `.gitignore`.
