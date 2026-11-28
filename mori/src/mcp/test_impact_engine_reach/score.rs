use super::*;

const RULE_SUITES: Index = 2;

impl Reach<'_> {
    fn trigger(
        &self,
        triggers: &mut Scratch<Trigger>,
        suite: Text,
        reached: bool,
        changed: bool,
        rule: bool,
    ) -> Checked<()> {
        if self.runner_suite(unsafe { bytes(suite) }) {
            self.allocate(triggers.push(Trigger {
                suite,
                reached,
                changed,
                rule,
            }))?;
        }
        Ok(())
    }

    fn reach_rows(
        &self,
        walk: &Walk,
        fixture_cases: &[bool],
        rows: &mut Scratch<ReachRow>,
        triggers: &mut Scratch<Trigger>,
    ) -> Checked<()> {
        let changed = borrowed(|count| unsafe {
            cbm_ti_seeds_changed_cases(self.input.seeds, count)
        });
        let mut row_ids = Scratch::default();
        let mut ids = Scratch::default();
        for (index, case) in self.cases.iter().enumerate() {
            let file = unsafe { bytes(case.file) };
            let name = unsafe { bytes(case.name) };
            ids.clear();
            self.named(case.file, case.name, case.start_line, &mut ids)?;
            let reached = ids.iter().any(|&id| walk.reached(id));
            let changed =
                fixture_cases[index] || changed.contains(&(index as Index));

            // A macro may register the case in any suite in the file.
            let whole_file = self.suites.iter().any(|suite| {
                (suite.macro_registrations || suite.uncertain)
                    && unsafe { bytes(suite.file) } == file
            });
            if whole_file {
                if reached || changed {
                    for suite in self
                        .suites
                        .iter()
                        .filter(|suite| unsafe { bytes(suite.file) } == file)
                    {
                        self.trigger(
                            triggers, suite.name, reached, changed, false,
                        )?;
                    }
                }
                continue;
            }

            let id = match &*ids {
                [id] => *id,
                _ => 0,
            };
            let previous = rows.iter().position(|row| unsafe {
                bytes(row.file) == file && bytes(row.test) == name
            });
            if let Some(at) = previous {
                // Two definitions with the same name can only be narrowed
                // when the graph identifies them as variants of one node.
                rows[at].mapped &= id != 0 && row_ids[at] == id;
                rows[at].reached |= reached;
                rows[at].changed |= changed;
            } else {
                self.allocate(row_ids.push(id))?;
                self.allocate(rows.push(ReachRow {
                    file: case.file,
                    test: case.name,
                    mapped: ids.len() == 1,
                    reached,
                    changed,
                }))?;
            }
        }
        Ok(())
    }

    fn suite_triggers(
        &self,
        walk: &Walk,
        triggers: &mut Scratch<Trigger>,
    ) -> Checked<()> {
        let changed = borrowed(|count| unsafe {
            cbm_ti_seeds_changed_suites(self.input.seeds, count)
        });
        let mut ids = Scratch::default();
        for (index, suite) in self.suites.iter().enumerate() {
            ids.clear();
            self.named(suite.file, suite.name, suite.start_line, &mut ids)?;
            let reached = ids.iter().any(|&id| walk.reached(id));
            let changed = changed.contains(&(index as Index));
            if reached || changed {
                self.trigger(triggers, suite.name, reached, changed, false)?;
            }
        }
        Ok(())
    }

    fn rule_triggers(&self, triggers: &mut Scratch<Trigger>) -> Checked<()> {
        let matches =
            unsafe { view(self.input.matches, self.input.match_count as usize) };
        for matched in matches {
            if self.rules[matched.rule_index as usize].action != RULE_SUITES {
                continue;
            }
            for runner in self.runners {
                let mut selected = false;
                let ok = unsafe {
                    cbm_test_policy_rule_selects_suite(
                        self.input.policy,
                        matched.rule_index,
                        runner.name,
                        runner.perf,
                        &mut selected,
                    )
                };
                if !ok {
                    return self.fallback(
                        Fallback::PolicyEvaluationFailed,
                        c"suite rule evaluation failed".as_ptr(),
                    );
                }
                if selected {
                    self.trigger(triggers, runner.name, false, false, true)?;
                }
            }
        }
        Ok(())
    }

    fn header_triggers(&self, triggers: &mut Scratch<Trigger>) -> Checked<()> {
        let headers = borrowed(|count| unsafe {
            cbm_ti_seeds_file_level_headers(self.input.seeds, count)
        });
        if headers.is_empty() {
            return Ok(());
        }
        let files = unsafe { cbm_ti_source_file_count(self.input.source) };
        let mut marked = Scratch::default();
        for _ in 0..files {
            self.allocate(marked.push(false))?;
        }
        for &header in headers {
            let file = self.source_find(header);
            if file >= 0 {
                marked[file as usize] = true;
            }
        }
        if !marked.is_empty() {
            mark_written(&marked);
            unsafe {
                cbm_ti_source_include_closure(
                    self.input.source,
                    marked.as_mut_ptr(),
                )
            };
        }
        // A file-level header change runs every suite that includes it.
        for suite in self.suites {
            let file = self.source_find(suite.file);
            if file >= 0 && marked[file as usize] {
                self.trigger(triggers, suite.name, true, false, false)?;
            }
        }
        Ok(())
    }

    pub(super) fn score(
        &self,
        walk: &Walk,
        fixture_cases: &[bool],
    ) -> Checked<()> {
        let mut rows = Scratch::default();
        let mut triggers = Scratch::default();
        self.reach_rows(walk, fixture_cases, &mut rows, &mut triggers)?;
        self.suite_triggers(walk, &mut triggers)?;
        self.rule_triggers(&mut triggers)?;
        self.header_triggers(&mut triggers)?;
        let (rows, row_count) = rows.into_raw_parts();
        let (triggers, trigger_count) = triggers.into_raw_parts();
        unsafe {
            cbm_test_impact_reach_publish(
                self.input.context,
                rows,
                row_count,
                triggers,
                trigger_count,
            )
        };
        Ok(())
    }
}
