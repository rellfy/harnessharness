In no specific order:
1. Small functions; single responsibility.
2. Avoid indentation hell, prefer to return early than to branch out.  
3. Comments capitalise the first letter, and end with full stop.  
4. Comments go above the line, not at the end of it.  
5. Document by code clarity and explicitness. Self-drescritive names.
6. Avoid blank lines inside functions.  
7. SQL tables are named as singular, not plural.
8. `bool` vars should be named as questions, e.g. `is_set` instead of `set`. Functions returning `bool` should be worded as questions with a verb prefix, e.g. `check_is_set` or `get_is_set`.
9. Escape long strings with \ and a line break to keep them readable.
10. For Rust: prefer `match` over `if` for "ternary operations".
11. For Rust: prefer to import the direct type rather than access as module::Type.
12. Source code files should read in order of importance: high-level, public functions go at the top, and low-level, private util functions go at the bottom. Consts, statics and structs go at the top.
13. Comments are an anti-pattern. Avoid them. Refer to #5.

