-- dropcap.lua
-- This filter looks for a span with class "dropcap", e.g. [T]{.dropcap}
-- and converts it into the appropriate LaTeX \lettrine command for PDF output,
-- or a styled span for HTML output.

function Span(el)
  if el.classes:includes("dropcap") then
    local letter = pandoc.utils.stringify(el)

    if FORMAT == "latex" or FORMAT == "pdf" then
      -- In LaTeX, \lettrine{FirstLetter}{RestOfWord}
      -- By wrapping only the first letter in the span like [W]{.dropcap}e
      -- we can emit \lettrine{W}{}e and LaTeX handles it perfectly.
      return pandoc.RawInline('latex', '\\lettrine{' .. letter .. '}{}')
    elseif FORMAT == "html" then
      return pandoc.RawInline('html', '<span class="dropcap" style="float: left; font-size: 3em; line-height: 0.5; margin-right: 0.1em; top: -1.5em; color: var(--bs-primary);">' .. letter .. '</span>')
    end
  end
end
