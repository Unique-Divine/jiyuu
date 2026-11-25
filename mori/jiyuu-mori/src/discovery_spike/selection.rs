//! Selection precedence shared by ordinary discovery and bounded admission.

// The enum values match discover.h. Each query runs only if its result can
// affect the decision, so paths with an early skip do not run later matchers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub(super) enum Check {
    CacheTree,
    HardcodedDir,
    SafetyCore,
    IgnoredSuffix,
    SkipFilename,
    FastPattern,
    Tracked,
    ParentIgnored,
    GitIgnore,
    GlobalIgnore,
    CbmIgnore,
    OverSize,
}

#[derive(Debug, Eq, PartialEq)]
#[repr(C)]
pub(super) enum Decision {
    Keep,
    KeepTrackedDir,
    SkipDir,
    IgnoredSuffix,
    SkipName,
    FastPattern,
    GitIgnore,
    CbmIgnore,
    SizeCap,
}

pub(super) fn directory(mut check: impl FnMut(Check) -> i32) -> Decision {
    if check(Check::CacheTree) > 0 {
        return Decision::SkipDir;
    }
    let hardcoded = check(Check::HardcodedDir) > 0;
    if hardcoded && check(Check::SafetyCore) > 0 {
        return Decision::SkipDir;
    }
    let cbm = check(Check::CbmIgnore);
    if (hardcoded && cbm >= 0) || cbm > 0 {
        return Decision::SkipDir;
    }
    let git_ignored = check(Check::ParentIgnored) > 0
        || check(Check::GitIgnore) > 0
        || (cbm >= 0 && check(Check::GlobalIgnore) > 0);
    if !git_ignored {
        Decision::Keep
    } else if check(Check::Tracked) > 0 {
        Decision::KeepTrackedDir
    } else {
        Decision::SkipDir
    }
}

pub(super) fn file(mut check: impl FnMut(Check) -> i32) -> Decision {
    if check(Check::IgnoredSuffix) > 0 {
        return Decision::IgnoredSuffix;
    }
    if check(Check::SkipFilename) > 0 {
        return Decision::SkipName;
    }
    if check(Check::FastPattern) > 0 {
        return Decision::FastPattern;
    }
    let untracked = check(Check::Tracked) == 0;
    if untracked
        && (check(Check::ParentIgnored) > 0 || check(Check::GitIgnore) > 0)
    {
        return Decision::GitIgnore;
    }
    let cbm = check(Check::CbmIgnore);
    if cbm > 0 {
        return Decision::CbmIgnore;
    }
    if check(Check::OverSize) > 0 {
        return Decision::SizeCap;
    }
    if untracked && cbm >= 0 && check(Check::GlobalIgnore) > 0 {
        return Decision::GitIgnore;
    }
    Decision::Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(facts: &[(Check, i32)]) -> impl FnMut(Check) -> i32 + '_ {
        |check| {
            facts
                .iter()
                .find(|(key, _)| *key == check)
                .map_or(0, |(_, value)| *value)
        }
    }

    #[test]
    fn file_reports_the_first_builtin_skip() {
        let mut checks = Vec::new();
        let decision = file(|check| {
            checks.push(check);
            i32::from(
                check == Check::SkipFilename || check == Check::FastPattern,
            )
        });
        assert_eq!(decision, Decision::SkipName);
        assert_eq!(checks, [Check::IgnoredSuffix, Check::SkipFilename]);
    }

    #[test]
    fn tracked_files_bypass_only_git_rules() {
        let git = [
            (Check::Tracked, 1),
            (Check::ParentIgnored, 1),
            (Check::GitIgnore, 1),
            (Check::GlobalIgnore, 1),
        ];
        assert_eq!(file(facts(&git)), Decision::Keep);
        assert_eq!(
            file(facts(&[&git[..], &[(Check::OverSize, 1)]].concat())),
            Decision::SizeCap
        );
        assert_eq!(
            file(facts(&[&git[..], &[(Check::CbmIgnore, 1)]].concat())),
            Decision::CbmIgnore
        );
    }

    #[test]
    fn cbm_negation_overrides_global_ignore_but_leaves_local_ignore() {
        let global = [(Check::CbmIgnore, -1), (Check::GlobalIgnore, 1)];
        assert_eq!(file(facts(&global)), Decision::Keep);
        assert_eq!(directory(facts(&global)), Decision::Keep);
        assert_eq!(
            file(facts(&[(Check::CbmIgnore, -1), (Check::GitIgnore, 1)])),
            Decision::GitIgnore
        );
        assert_eq!(
            directory(facts(&[(Check::CbmIgnore, -1), (Check::GitIgnore, 1)])),
            Decision::SkipDir
        );
    }

    #[test]
    fn tracking_allows_descent_only_through_git_ignored_directories() {
        assert_eq!(
            directory(facts(&[(Check::ParentIgnored, 1), (Check::Tracked, 1)])),
            Decision::KeepTrackedDir
        );
        assert_eq!(
            directory(facts(&[(Check::ParentIgnored, 1)])),
            Decision::SkipDir
        );
        assert_eq!(
            directory(facts(&[(Check::CbmIgnore, 1), (Check::Tracked, 1)])),
            Decision::SkipDir
        );
    }

    #[test]
    fn cbm_negation_does_not_unprune_cache_or_safety_core() {
        assert_eq!(
            directory(facts(&[
                (Check::HardcodedDir, 1),
                (Check::CbmIgnore, -1)
            ])),
            Decision::Keep
        );
        assert_eq!(
            directory(facts(&[
                (Check::HardcodedDir, 1),
                (Check::SafetyCore, 1),
                (Check::CbmIgnore, -1),
            ])),
            Decision::SkipDir
        );
        assert_eq!(
            directory(facts(&[(Check::CacheTree, 1), (Check::CbmIgnore, -1)])),
            Decision::SkipDir
        );
    }
}
