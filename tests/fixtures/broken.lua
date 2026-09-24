-- Test fixture: a syntax error; the parser stops at the "}" on line 5.
return {
  title = "Broken",
  update = function(
}
