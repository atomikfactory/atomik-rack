Inputs that once made a fuzz target fail, one folder per target. Each is replayed from then on
(scripts/fuzz-seed.sh adds them to the corpus). Commit the input a failing run uploaded, with
the fix that makes it pass.
