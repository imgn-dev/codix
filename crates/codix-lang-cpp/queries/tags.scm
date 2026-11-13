; Written for codix after tree-sitter-cpp 0.23.4, queries/tags.scm, which
; captures a function's declarator rather than the whole definition; here
; the definition is the whole function, class, struct, enum or typedef. The
; C patterns come first; later patterns win for the same node.

(function_definition
  declarator: [
    (function_declarator
      declarator: (identifier) @name)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (identifier) @name))
    (pointer_declarator
      declarator: (pointer_declarator
        declarator: (function_declarator
          declarator: (identifier) @name)))
  ]) @definition.function

; A prototype: the function's name, without its body.
(declaration
  declarator: [
    (function_declarator
      declarator: (identifier) @name)
    (pointer_declarator
      declarator: (function_declarator
        declarator: (identifier) @name))
  ]) @definition.declaration

(struct_specifier
  name: (type_identifier) @name
  body: (_)) @definition.struct

(union_specifier
  name: (type_identifier) @name
  body: (_)) @definition.struct

(enum_specifier
  name: (type_identifier) @name
  body: (_)) @definition.enum

(enumerator
  name: (identifier) @name) @definition.enum_member

(field_declaration
  declarator: [
    (field_identifier) @name
    (pointer_declarator
      declarator: (field_identifier) @name)
    (array_declarator
      declarator: (field_identifier) @name)
  ]) @definition.field

(type_definition
  declarator: [
    (type_identifier) @name
    (pointer_declarator
      declarator: (type_identifier) @name)
  ]) @definition.type_alias

(preproc_def
  name: (identifier) @name) @definition.macro

(preproc_function_def
  name: (identifier) @name) @definition.macro

(translation_unit
  (declaration
    declarator: [
      (identifier) @name
      (init_declarator
        declarator: (identifier) @name)
      (init_declarator
        declarator: (pointer_declarator
          declarator: (identifier) @name))
      (init_declarator
        declarator: (array_declarator
          declarator: (identifier) @name))
      (pointer_declarator
        declarator: (identifier) @name)
      (array_declarator
        declarator: (identifier) @name)
      (pointer_declarator
        declarator: (array_declarator
          declarator: (identifier) @name))
    ]) @definition.variable)

(call_expression
  function: (identifier) @name) @reference.call

(class_specifier
  name: (type_identifier) @name
  body: (_)) @definition.class

(namespace_definition
  name: (namespace_identifier) @name) @definition.namespace

(alias_declaration
  name: (type_identifier) @name) @definition.type_alias

; Members declared or defined in a class body.

(field_declaration_list
  [
    (function_definition
      declarator: (function_declarator
        declarator: [(field_identifier) (destructor_name) (operator_name)] @name)) @definition.method
    (field_declaration
      declarator: (function_declarator
        declarator: [(field_identifier) (destructor_name) (operator_name)] @name)) @definition.method
    (declaration
      declarator: (function_declarator
        declarator: [(field_identifier) (destructor_name) (operator_name)] @name)) @definition.method
  ])

(class_specifier
  name: (type_identifier) @class
  body: (field_declaration_list
    [
      (function_definition
        declarator: (function_declarator
          declarator: (identifier) @name)) @definition.constructor
      (declaration
        declarator: (function_declarator
          declarator: (identifier) @name)) @definition.constructor
    ])
  (#eq? @name @class))

; Members defined outside their class: `void Point::move()`.

(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier
      scope: [
        (namespace_identifier) @parent
        (template_type
          name: (type_identifier) @parent)
      ]
      name: [(identifier) (field_identifier) (destructor_name) (operator_name)] @name))) @definition.method

(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier
      scope: (namespace_identifier) @parent
      name: (identifier) @name))
  (#eq? @name @parent)) @definition.constructor

; The same, with the class itself in a namespace: `geo::Shape::area`.

(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier
      scope: (namespace_identifier) @parent
      name: (qualified_identifier
        scope: [
          (namespace_identifier) @parent
          (template_type
            name: (type_identifier) @parent)
        ]
        name: [(identifier) (field_identifier) (destructor_name) (operator_name)] @name)))) @definition.method

(function_definition
  declarator: (function_declarator
    declarator: (qualified_identifier
      scope: (namespace_identifier) @parent
      name: (qualified_identifier
        scope: (namespace_identifier) @parent @class
        name: (identifier) @name)))
  (#eq? @name @class)) @definition.constructor

(call_expression
  function: (qualified_identifier
    name: (identifier) @name)) @reference.call

; Added for codix: includes, as written.

(preproc_include path: (string_literal) @include)

((preproc_include path: (system_lib_string) @include)
  (#set! include.system))

; Added for codix: supertypes, as written.

(class_specifier
  (base_class_clause [(type_identifier) (qualified_identifier) (template_type)] @supertype)) @supertypes

(struct_specifier
  (base_class_clause [(type_identifier) (qualified_identifier) (template_type)] @supertype)) @supertypes
