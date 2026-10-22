import tsParser from '@typescript-eslint/parser';
import { importX } from 'eslint-plugin-import-x';
import jsdoc from 'eslint-plugin-jsdoc';
import stylistic from '@stylistic/eslint-plugin';
import noComments from 'eslint-plugin-no-comments';
export default [
  {
    files: [
      'types/**/*.*',
      'gen-types/**/*.*',
    ],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        sourceType: 'module'
      }
    },
    ignores: ['dist/**', 'node_modules/**', 'src/**'],
    plugins: {
      jsdoc,
      'import-x': importX,
      '@stylistic': stylistic,
      'no-comments': noComments
    },
    rules: {
      'jsdoc/multiline-blocks': [
        'error',
        {
          noSingleLineBlocks: true
        }
      ],
      'jsdoc/check-alignment': 'error',
      'no-use-before-define': 'off',
      '@typescript-eslint/no-use-before-define': 'off',
      'import-x/first': 'error',
      '@stylistic/indent': ['error', 2],
      'no-comments/disallowComments': 'error',
      '@stylistic/no-multiple-empty-lines': ['error', { max: 0, maxBOF: 0, maxEOF: 0 }]
    }
  }
];
