use nickel_plane::{reconcile::PollEvents, Result};
use std::time::Duration;

/// Wait between serve cycles. Clock callbacks keep scheduling tests deterministic.
pub(crate) fn wait_for_resync(
    interval: Duration,
    mut cancelled: impl FnMut() -> bool,
    mut poll: impl FnMut() -> Result<PollEvents>,
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
) -> Result<()> {
    let mut hint = PollEvents::Quiet;
    while elapsed() < interval && !cancelled() {
        if hint == PollEvents::Quiet {
            hint = poll()?;
        }
        // A changed/expired watch coalesces the entire instance's next cycle.
        // An unavailable watch falls back to periodic observation without
        // repeatedly contacting the failed endpoint during this wait window.
        let deadline = if hint == PollEvents::Changed {
            interval.min(Duration::from_secs(1))
        } else {
            interval
        };
        let remaining = deadline.saturating_sub(elapsed());
        if remaining.is_zero() || cancelled() {
            break;
        }
        sleep(remaining.min(Duration::from_millis(250)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nickel_plane::Error;
    use std::cell::Cell;

    #[test]
    fn immediate_events_cannot_run_more_than_one_cycle_per_second() -> Result<()> {
        let clock = Cell::new(Duration::ZERO);
        let polls = Cell::new(0);
        for _ in 0..3 {
            let start = clock.get();
            wait_for_resync(
                Duration::from_secs(30),
                || false,
                || {
                    polls.set(polls.get() + 1);
                    Ok(PollEvents::Changed)
                },
                || clock.get() - start,
                |duration| clock.set(clock.get() + duration),
            )?;
        }
        assert_eq!(clock.get(), Duration::from_secs(3));
        assert_eq!(polls.get(), 3);
        Ok(())
    }

    #[test]
    fn unavailable_watch_waits_for_periodic_resync_then_can_recover() -> Result<()> {
        let clock = Cell::new(Duration::ZERO);
        let polls = Cell::new(0);
        for hint in [PollEvents::Unavailable, PollEvents::Changed] {
            let start = clock.get();
            wait_for_resync(
                Duration::from_secs(30),
                || false,
                || {
                    polls.set(polls.get() + 1);
                    Ok(hint)
                },
                || clock.get() - start,
                |duration| clock.set(clock.get() + duration),
            )?;
            if hint == PollEvents::Unavailable {
                assert_eq!(clock.get(), Duration::from_secs(30));
                assert_eq!(polls.get(), 1);
            }
        }
        assert_eq!(clock.get(), Duration::from_secs(31));
        assert_eq!(polls.get(), 2);
        Ok(())
    }

    #[test]
    fn quiet_polls_are_spaced_and_preserve_the_periodic_deadline() -> Result<()> {
        let clock = Cell::new(Duration::ZERO);
        let mut polled_at = Vec::new();
        wait_for_resync(
            Duration::from_secs(2),
            || false,
            || {
                polled_at.push(clock.get());
                Ok(PollEvents::Quiet)
            },
            || clock.get(),
            |duration| clock.set(clock.get() + duration),
        )?;
        assert_eq!(clock.get(), Duration::from_secs(2));
        assert_eq!(polled_at.len(), 8);
        assert!(polled_at
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= Duration::from_millis(250)));
        Ok(())
    }

    #[test]
    fn cancellation_interrupts_event_coalescing_and_watch_failure_waits() -> Result<()> {
        for hint in [
            PollEvents::Changed,
            PollEvents::Unavailable,
            PollEvents::Quiet,
        ] {
            let clock = Cell::new(Duration::ZERO);
            let polls = Cell::new(0);
            wait_for_resync(
                Duration::from_secs(30),
                || clock.get() >= Duration::from_millis(250),
                || {
                    polls.set(polls.get() + 1);
                    Ok(hint)
                },
                || clock.get(),
                |duration| clock.set(clock.get() + duration),
            )?;
            assert_eq!(clock.get(), Duration::from_millis(250));
            assert_eq!(polls.get(), 1);
        }
        Ok(())
    }

    #[test]
    fn polling_time_counts_toward_periodic_and_coalescing_deadlines() -> Result<()> {
        for (hint, poll_time, expected) in [
            (PollEvents::Quiet, 900, 1000),
            (PollEvents::Changed, 1500, 1500),
            (PollEvents::Unavailable, 1500, 1500),
        ] {
            let clock = Cell::new(Duration::ZERO);
            let polls = Cell::new(0);
            wait_for_resync(
                Duration::from_secs(1),
                || false,
                || {
                    polls.set(polls.get() + 1);
                    clock.set(clock.get() + Duration::from_millis(poll_time));
                    Ok(hint)
                },
                || clock.get(),
                |duration| clock.set(clock.get() + duration),
            )?;
            assert_eq!(clock.get(), Duration::from_millis(expected));
            assert_eq!(polls.get(), 1);
        }
        Ok(())
    }

    #[test]
    fn cancellation_prevents_starting_a_watch() -> Result<()> {
        let polls = Cell::new(0);
        let sleeps = Cell::new(0);
        wait_for_resync(
            Duration::from_secs(30),
            || true,
            || {
                polls.set(polls.get() + 1);
                Ok(PollEvents::Quiet)
            },
            || Duration::ZERO,
            |_| sleeps.set(sleeps.get() + 1),
        )?;
        assert_eq!(polls.get(), 0);
        assert_eq!(sleeps.get(), 0);
        Ok(())
    }

    #[test]
    fn store_errors_remain_fatal() {
        let sleeps = Cell::new(0);
        let result = wait_for_resync(
            Duration::from_secs(30),
            || false,
            || Err(Error::Conflict("store unavailable".into())),
            || Duration::ZERO,
            |_| sleeps.set(sleeps.get() + 1),
        );
        assert!(matches!(result, Err(Error::Conflict(_))));
        assert_eq!(sleeps.get(), 0);
    }
}
