; Written for codix: headings. Each heading's section nests the sections
; under it, so a qualified name is the heading path.

(section
  (atx_heading
    heading_content: (inline) @name)) @definition.module

;; Setext headings (underlined) open no section of their own.
(setext_heading
  heading_content: (paragraph) @name) @definition.module
