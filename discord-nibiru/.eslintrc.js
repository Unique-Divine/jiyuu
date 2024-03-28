module.exports = {
  "env": {
    "browser": true,
    "es6": true,
    "node": true
  },
  "extends": [
    "eslint:recommended",
    "plugin:react/recommended",
    "plugin:@typescript-eslint/eslint-recommended",
    "prettier",
    "prettier",
  ],
  "globals": {
    "Atomics": "readonly",
    "SharedArrayBuffer": "readonly"
  },
  "parser": "@typescript-eslint/parser",
  "parserOptions": {
    "ecmaFeatures": {
      "jsx": true
    },
    "ecmaVersion": 2018,
    "sourceType": "module"
  },
  "plugins": [
    "react",
    "@typescript-eslint"
  ],
  "rules": {
    "indent": [
      "warn",
      2,
      { "SwitchCase": 1, "ignoredNodes": [
        "ArrowFunctionExpression > BlockStatement",
        "NoSubstitutionTemplateLiteral",
        "TemplateLiteral",
        "TSTypeAliasDeclaration *"
      ]}
    ],
    "linebreak-style": [
      "off",
      "unix"
    ],
    "quotes": [
      "error",
      "double", {"allowTemplateLiterals": true, "avoidEscape": true}
    ],
    "react/no-children-prop": ["off"],
    "no-unused-vars": ["off", { "args": "none", "argsIgnorePattern": "^_", "vars": "local" }],
    "@typescript-eslint/no-unused-vars": ["warn", { 
      "args": "none", 
      "argsIgnorePattern": "^_", 
      "vars": "local"
    }],
    "semi": [
      "error",
      "never"
    ]
  }
}
