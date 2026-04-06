# Rule philosophy

The project is built for repos owned by me personally, so there's no CODEOWNER concern, and repos are specifically intended to be operated by one person.

However, repos *should* be configured in such a way that it's easy for consumers to see what they're consuming and to detect foul play.
For example:

* force-pushing to master can obfuscate history in confusing ways, so it's banned.
* opaque binary artifacts should be attested in some way, e.g. with GitHub Attestations.

And it should be easy for me to fire-and-forget commits to the repos.
For example:

* every pipeline should have an "all required checks complete" stage, which auto-merging waits for; this check must be [a specific one which actually enforces its passing](https://github.com/G-Research/common-actions/tree/19d7281a0f9f83e13c78f99a610dbc80fc59ba3b/check-required-lite).

