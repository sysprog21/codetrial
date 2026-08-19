export const judges = {
  "valid-parentheses": {
    "kind": "function",
    "entry": "isValid",
    "checker": "exact",
    "cases": [
      {
        "label": "s=\"()\"",
        "input": [
          "()"
        ],
        "expected": true
      },
      {
        "label": "s=\"()[]{}\"",
        "input": [
          "()[]{}"
        ],
        "expected": true
      },
      {
        "label": "mismatched type: s=\"(]\"",
        "input": [
          "(]"
        ],
        "expected": false
      },
      {
        "label": "interleaved: s=\"([)]\"",
        "input": [
          "([)]"
        ],
        "expected": false
      },
      {
        "label": "nested: s=\"{[]}\"",
        "input": [
          "{[]}"
        ],
        "expected": true
      },
      {
        "label": "unclosed: s=\"(\"",
        "input": [
          "("
        ],
        "expected": false
      },
      {
        "label": "close first: s=\")(\"",
        "input": [
          ")("
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "boolean"
  },
  "merge-sorted-array": {
    "kind": "function",
    "entry": "merge",
    "checker": "exact",
    "outputParam": 0,
    "cases": [
      {
        "label": "interleaved tail space",
        "input": [
          [
            1,
            2,
            3,
            0,
            0,
            0
          ],
          3,
          [
            2,
            5,
            6
          ],
          3
        ],
        "expected": [
          1,
          2,
          2,
          3,
          5,
          6
        ]
      },
      {
        "label": "nums2 empty",
        "input": [
          [
            1
          ],
          1,
          [],
          0
        ],
        "expected": [
          1
        ]
      },
      {
        "label": "nums1 empty prefix",
        "input": [
          [
            0
          ],
          0,
          [
            1
          ],
          1
        ],
        "expected": [
          1
        ]
      },
      {
        "label": "duplicates and negatives",
        "input": [
          [
            -3,
            -1,
            0,
            0,
            0
          ],
          2,
          [
            -2,
            -1,
            2
          ],
          3
        ],
        "expected": [
          -3,
          -2,
          -1,
          -1,
          2
        ]
      }
    ],
    "paramNames": [
      "nums1",
      "m",
      "nums2",
      "n"
    ],
    "paramTypes": [
      "integer[]",
      "integer",
      "integer[]",
      "integer"
    ],
    "returnType": "void"
  },
  "remove-element": {
    "kind": "function",
    "entry": "removeElement",
    "checker": "arrayBag",
    "outputPrefixParam": 0,
    "cases": [
      {
        "label": "remove all 3s",
        "input": [
          [
            3,
            2,
            2,
            3
          ],
          3
        ],
        "expected": [
          2,
          2
        ]
      },
      {
        "label": "remove scattered 2s",
        "input": [
          [
            0,
            1,
            2,
            2,
            3,
            0,
            4,
            2
          ],
          2
        ],
        "expected": [
          0,
          1,
          3,
          0,
          4
        ]
      },
      {
        "label": "value absent keeps all",
        "input": [
          [
            1,
            2,
            3
          ],
          4
        ],
        "expected": [
          1,
          2,
          3
        ]
      },
      {
        "label": "remove every element",
        "input": [
          [
            7,
            7,
            7
          ],
          7
        ],
        "expected": []
      }
    ],
    "paramNames": [
      "nums",
      "val"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer"
  },
  "remove-duplicates-from-sorted-array": {
    "kind": "function",
    "entry": "removeDuplicates",
    "checker": "exact",
    "outputPrefixParam": 0,
    "cases": [
      {
        "label": "one duplicate pair",
        "input": [
          [
            1,
            1,
            2
          ]
        ],
        "expected": [
          1,
          2
        ]
      },
      {
        "label": "several duplicate runs",
        "input": [
          [
            0,
            0,
            1,
            1,
            1,
            2,
            2,
            3,
            3,
            4
          ]
        ],
        "expected": [
          0,
          1,
          2,
          3,
          4
        ]
      },
      {
        "label": "already unique",
        "input": [
          [
            -2,
            -1,
            0,
            3
          ]
        ],
        "expected": [
          -2,
          -1,
          0,
          3
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "remove-duplicates-from-sorted-array-ii": {
    "kind": "function",
    "entry": "removeDuplicates",
    "checker": "exact",
    "outputPrefixParam": 0,
    "cases": [
      {
        "label": "trim triple to two",
        "input": [
          [
            1,
            1,
            1,
            2,
            2,
            3
          ]
        ],
        "expected": [
          1,
          1,
          2,
          2,
          3
        ]
      },
      {
        "label": "mixed duplicate runs",
        "input": [
          [
            0,
            0,
            1,
            1,
            1,
            1,
            2,
            3,
            3
          ]
        ],
        "expected": [
          0,
          0,
          1,
          1,
          2,
          3,
          3
        ]
      },
      {
        "label": "single repeated value",
        "input": [
          [
            5,
            5,
            5,
            5
          ]
        ],
        "expected": [
          5,
          5
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "majority-element": {
    "kind": "function",
    "entry": "majorityElement",
    "checker": "exact",
    "cases": [
      {
        "label": "majority at both ends",
        "input": [
          [
            3,
            2,
            3
          ]
        ],
        "expected": 3
      },
      {
        "label": "majority after noise",
        "input": [
          [
            2,
            2,
            1,
            1,
            1,
            2,
            2
          ]
        ],
        "expected": 2
      },
      {
        "label": "single item",
        "input": [
          [
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "not the first value",
        "input": [
          [
            6,
            5,
            5
          ]
        ],
        "expected": 5
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "rotate-array": {
    "kind": "function",
    "entry": "rotate",
    "checker": "exact",
    "outputParam": 0,
    "cases": [
      {
        "label": "right rotate by three",
        "input": [
          [
            1,
            2,
            3,
            4,
            5,
            6,
            7
          ],
          3
        ],
        "expected": [
          5,
          6,
          7,
          1,
          2,
          3,
          4
        ]
      },
      {
        "label": "negative values",
        "input": [
          [
            -1,
            -100,
            3,
            99
          ],
          2
        ],
        "expected": [
          3,
          99,
          -1,
          -100
        ]
      },
      {
        "label": "k larger than length",
        "input": [
          [
            1,
            2
          ],
          3
        ],
        "expected": [
          2,
          1
        ]
      },
      {
        "label": "k is zero",
        "input": [
          [
            4,
            5,
            6
          ],
          0
        ],
        "expected": [
          4,
          5,
          6
        ]
      }
    ],
    "paramNames": [
      "nums",
      "k"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "void"
  },
  "best-time-to-buy-and-sell-stock": {
    "kind": "function",
    "entry": "maxProfit",
    "checker": "exact",
    "cases": [
      {
        "label": "buy low sell later",
        "input": [
          [
            7,
            1,
            5,
            3,
            6,
            4
          ]
        ],
        "expected": 5
      },
      {
        "label": "decreasing prices",
        "input": [
          [
            7,
            6,
            4,
            3,
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "profit before later dip",
        "input": [
          [
            2,
            4,
            1
          ]
        ],
        "expected": 2
      },
      {
        "label": "best sell after middle valley",
        "input": [
          [
            3,
            2,
            6,
            5,
            0,
            3
          ]
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "prices"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "best-time-to-buy-and-sell-stock-ii": {
    "kind": "function",
    "entry": "maxProfit",
    "checker": "exact",
    "cases": [
      {
        "label": "two profitable swings",
        "input": [
          [
            7,
            1,
            5,
            3,
            6,
            4
          ]
        ],
        "expected": 7
      },
      {
        "label": "steady climb",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ]
        ],
        "expected": 4
      },
      {
        "label": "decreasing prices",
        "input": [
          [
            7,
            6,
            4,
            3,
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "multiple small rises",
        "input": [
          [
            1,
            2,
            1,
            2,
            1,
            2
          ]
        ],
        "expected": 3
      }
    ],
    "paramNames": [
      "prices"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "jump-game": {
    "kind": "function",
    "entry": "canJump",
    "checker": "exact",
    "cases": [
      {
        "label": "can hop over tail",
        "input": [
          [
            2,
            3,
            1,
            1,
            4
          ]
        ],
        "expected": true
      },
      {
        "label": "zero trap",
        "input": [
          [
            3,
            2,
            1,
            0,
            4
          ]
        ],
        "expected": false
      },
      {
        "label": "single index",
        "input": [
          [
            0
          ]
        ],
        "expected": true
      },
      {
        "label": "reachable exact landing",
        "input": [
          [
            2,
            0,
            0
          ]
        ],
        "expected": true
      },
      {
        "label": "late unreachable gap",
        "input": [
          [
            1,
            0,
            1,
            0
          ]
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "boolean"
  },
  "jump-game-ii": {
    "kind": "function",
    "entry": "jump",
    "checker": "exact",
    "cases": [
      {
        "label": "sample two jumps",
        "input": [
          [
            2,
            3,
            1,
            1,
            4
          ]
        ],
        "expected": 2
      },
      {
        "label": "zero inside reachable path",
        "input": [
          [
            2,
            3,
            0,
            1,
            4
          ]
        ],
        "expected": 2
      },
      {
        "label": "already at end",
        "input": [
          [
            0
          ]
        ],
        "expected": 0
      },
      {
        "label": "greedy window needs three",
        "input": [
          [
            1,
            2,
            1,
            1,
            1
          ]
        ],
        "expected": 3
      },
      {
        "label": "first jump can skip far",
        "input": [
          [
            4,
            1,
            1,
            3,
            1,
            1,
            1
          ]
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "h-index": {
    "kind": "function",
    "entry": "hIndex",
    "checker": "exact",
    "cases": [
      {
        "label": "mixed citation counts",
        "input": [
          [
            3,
            0,
            6,
            1,
            5
          ]
        ],
        "expected": 3
      },
      {
        "label": "small h only",
        "input": [
          [
            1,
            3,
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "all zero citations",
        "input": [
          [
            0,
            0,
            0
          ]
        ],
        "expected": 0
      },
      {
        "label": "h capped by paper count",
        "input": [
          [
            100,
            100
          ]
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "citations"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "insert-delete-getrandom-o1": {
    "kind": "class",
    "className": "RandomizedSet",
    "checker": "exact",
    "cases": [
      {
        "label": "sample singleton random",
        "input": [
          [
            "RandomizedSet",
            "insert",
            "remove",
            "insert",
            "remove",
            "getRandom",
            "insert",
            "getRandom"
          ],
          [
            [],
            [
              1
            ],
            [
              2
            ],
            [
              2
            ],
            [
              1
            ],
            [],
            [
              2
            ],
            []
          ]
        ],
        "expected": [
          null,
          true,
          false,
          true,
          true,
          2,
          false,
          2
        ]
      },
      {
        "label": "duplicate insert and remove",
        "input": [
          [
            "RandomizedSet",
            "insert",
            "insert",
            "remove",
            "remove"
          ],
          [
            [],
            [
              5
            ],
            [
              5
            ],
            [
              5
            ],
            [
              5
            ]
          ]
        ],
        "expected": [
          null,
          true,
          false,
          true,
          false
        ]
      },
      {
        "label": "removed value is not random",
        "input": [
          [
            "RandomizedSet",
            "insert",
            "insert",
            "remove",
            "getRandom"
          ],
          [
            [],
            [
              1
            ],
            [
              2
            ],
            [
              1
            ],
            []
          ]
        ],
        "expected": [
          null,
          true,
          true,
          true,
          2
        ]
      }
    ]
  },
  "product-of-array-except-self": {
    "kind": "function",
    "entry": "productExceptSelf",
    "checker": "exact",
    "cases": [
      {
        "label": "positive numbers",
        "input": [
          [
            1,
            2,
            3,
            4
          ]
        ],
        "expected": [
          24,
          12,
          8,
          6
        ]
      },
      {
        "label": "single zero",
        "input": [
          [
            -1,
            1,
            0,
            -3,
            3
          ]
        ],
        "expected": [
          0,
          0,
          9,
          0,
          0
        ]
      },
      {
        "label": "two zeros",
        "input": [
          [
            0,
            4,
            0
          ]
        ],
        "expected": [
          0,
          0,
          0
        ]
      },
      {
        "label": "negative product signs",
        "input": [
          [
            -1,
            2,
            -3,
            4
          ]
        ],
        "expected": [
          -24,
          12,
          -8,
          6
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer[]"
  },
  "gas-station": {
    "kind": "function",
    "entry": "canCompleteCircuit",
    "checker": "exact",
    "cases": [
      {
        "label": "start after deficit",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ],
          [
            3,
            4,
            5,
            1,
            2
          ]
        ],
        "expected": 3
      },
      {
        "label": "impossible total gas",
        "input": [
          [
            2,
            3,
            4
          ],
          [
            3,
            4,
            3
          ]
        ],
        "expected": -1
      },
      {
        "label": "wraparound start",
        "input": [
          [
            5,
            1,
            2,
            3,
            4
          ],
          [
            4,
            4,
            1,
            5,
            1
          ]
        ],
        "expected": 4
      },
      {
        "label": "single exact station",
        "input": [
          [
            1
          ],
          [
            1
          ]
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "gas",
      "cost"
    ],
    "paramTypes": [
      "integer[]",
      "integer[]"
    ],
    "returnType": "integer"
  },
  "candy": {
    "kind": "function",
    "entry": "candy",
    "checker": "exact",
    "cases": [
      {
        "label": "valley needs both sides",
        "input": [
          [
            1,
            0,
            2
          ]
        ],
        "expected": 5
      },
      {
        "label": "plateau is not greater",
        "input": [
          [
            1,
            2,
            2
          ]
        ],
        "expected": 4
      },
      {
        "label": "long rising then drop",
        "input": [
          [
            1,
            3,
            4,
            5,
            2
          ]
        ],
        "expected": 11
      },
      {
        "label": "plateau between slopes",
        "input": [
          [
            1,
            2,
            87,
            87,
            87,
            2,
            1
          ]
        ],
        "expected": 13
      },
      {
        "label": "single child",
        "input": [
          [
            5
          ]
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "ratings"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "trapping-rain-water": {
    "kind": "function",
    "entry": "trap",
    "checker": "exact",
    "cases": [
      {
        "label": "classic mixed bars",
        "input": [
          [
            0,
            1,
            0,
            2,
            1,
            0,
            1,
            3,
            2,
            1,
            2,
            1
          ]
        ],
        "expected": 6
      },
      {
        "label": "deep basin",
        "input": [
          [
            4,
            2,
            0,
            3,
            2,
            5
          ]
        ],
        "expected": 9
      },
      {
        "label": "flat bars",
        "input": [
          [
            1,
            1,
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "small bowl",
        "input": [
          [
            2,
            0,
            2
          ]
        ],
        "expected": 2
      },
      {
        "label": "right boundary lower",
        "input": [
          [
            5,
            4,
            1,
            2
          ]
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "height"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "roman-to-integer": {
    "kind": "function",
    "entry": "romanToInt",
    "checker": "exact",
    "cases": [
      {
        "label": "simple repeats",
        "input": [
          "III"
        ],
        "expected": 3
      },
      {
        "label": "mixed additive",
        "input": [
          "LVIII"
        ],
        "expected": 58
      },
      {
        "label": "compound subtractive",
        "input": [
          "MCMXCIV"
        ],
        "expected": 1994
      },
      {
        "label": "single subtractive pair",
        "input": [
          "IV"
        ],
        "expected": 4
      },
      {
        "label": "lower and upper subtractive",
        "input": [
          "XLIX"
        ],
        "expected": 49
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "integer"
  },
  "integer-to-roman": {
    "kind": "function",
    "entry": "intToRoman",
    "checker": "exact",
    "cases": [
      {
        "label": "large mixed value",
        "input": [
          3749
        ],
        "expected": "MMMDCCXLIX"
      },
      {
        "label": "mixed additive",
        "input": [
          58
        ],
        "expected": "LVIII"
      },
      {
        "label": "compound subtractive",
        "input": [
          1994
        ],
        "expected": "MCMXCIV"
      },
      {
        "label": "single subtractive pair",
        "input": [
          4
        ],
        "expected": "IV"
      },
      {
        "label": "hundreds tens and ones",
        "input": [
          944
        ],
        "expected": "CMXLIV"
      }
    ],
    "paramNames": [
      "num"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "string"
  },
  "length-of-last-word": {
    "kind": "function",
    "entry": "lengthOfLastWord",
    "checker": "exact",
    "cases": [
      {
        "label": "two words",
        "input": [
          "Hello World"
        ],
        "expected": 5
      },
      {
        "label": "trailing spaces",
        "input": [
          "   fly me   to   the moon  "
        ],
        "expected": 4
      },
      {
        "label": "last word at end",
        "input": [
          "luffy is still joyboy"
        ],
        "expected": 6
      },
      {
        "label": "single letter",
        "input": [
          "a"
        ],
        "expected": 1
      },
      {
        "label": "single word with leading spaces",
        "input": [
          "   single"
        ],
        "expected": 6
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "integer"
  },
  "longest-common-prefix": {
    "kind": "function",
    "entry": "longestCommonPrefix",
    "checker": "exact",
    "cases": [
      {
        "label": "shared short prefix",
        "input": [
          [
            "flower",
            "flow",
            "flight"
          ]
        ],
        "expected": "fl"
      },
      {
        "label": "no common first letter",
        "input": [
          [
            "dog",
            "racecar",
            "car"
          ]
        ],
        "expected": ""
      },
      {
        "label": "single string",
        "input": [
          [
            "alone"
          ]
        ],
        "expected": "alone"
      },
      {
        "label": "empty string in list",
        "input": [
          [
            "",
            "prefix"
          ]
        ],
        "expected": ""
      },
      {
        "label": "prefix is whole shortest word",
        "input": [
          [
            "inter",
            "internet",
            "internal"
          ]
        ],
        "expected": "inter"
      }
    ],
    "paramNames": [
      "strs"
    ],
    "paramTypes": [
      "string[]"
    ],
    "returnType": "string"
  },
  "reverse-words-in-a-string": {
    "kind": "function",
    "entry": "reverseWords",
    "checker": "exact",
    "cases": [
      {
        "label": "simple sentence",
        "input": [
          "the sky is blue"
        ],
        "expected": "blue is sky the"
      },
      {
        "label": "trim outer spaces",
        "input": [
          "  hello world  "
        ],
        "expected": "world hello"
      },
      {
        "label": "collapse internal spaces",
        "input": [
          "a good   example"
        ],
        "expected": "example good a"
      },
      {
        "label": "single word with spaces",
        "input": [
          "  solo  "
        ],
        "expected": "solo"
      },
      {
        "label": "digits are words",
        "input": [
          "one 2 three"
        ],
        "expected": "three 2 one"
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "string"
  },
  "zigzag-conversion": {
    "kind": "function",
    "entry": "convert",
    "checker": "exact",
    "cases": [
      {
        "label": "three rows",
        "input": [
          "PAYPALISHIRING",
          3
        ],
        "expected": "PAHNAPLSIIGYIR"
      },
      {
        "label": "four rows",
        "input": [
          "PAYPALISHIRING",
          4
        ],
        "expected": "PINALSIGYAHRPI"
      },
      {
        "label": "single row",
        "input": [
          "A",
          1
        ],
        "expected": "A"
      },
      {
        "label": "rows exceed length",
        "input": [
          "AB",
          5
        ],
        "expected": "AB"
      },
      {
        "label": "short cycle",
        "input": [
          "ABCDE",
          2
        ],
        "expected": "ACEBD"
      }
    ],
    "paramNames": [
      "s",
      "numRows"
    ],
    "paramTypes": [
      "string",
      "integer"
    ],
    "returnType": "string"
  },
  "find-the-index-of-the-first-occurrence-in-a-string": {
    "kind": "function",
    "entry": "strStr",
    "checker": "exact",
    "cases": [
      {
        "label": "match at start",
        "input": [
          "sadbutsad",
          "sad"
        ],
        "expected": 0
      },
      {
        "label": "not found",
        "input": [
          "leetcode",
          "leeto"
        ],
        "expected": -1
      },
      {
        "label": "later occurrence",
        "input": [
          "mississippi",
          "issip"
        ],
        "expected": 4
      },
      {
        "label": "overlapping prefix",
        "input": [
          "aaaaa",
          "bba"
        ],
        "expected": -1
      },
      {
        "label": "needle equals haystack",
        "input": [
          "abc",
          "abc"
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "haystack",
      "needle"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "integer"
  },
  "text-justification": {
    "kind": "function",
    "entry": "fullJustify",
    "checker": "exact",
    "cases": [
      {
        "label": "uneven spaces go left",
        "input": [
          [
            "This",
            "is",
            "an",
            "example",
            "of",
            "text",
            "justification."
          ],
          16
        ],
        "expected": [
          "This    is    an",
          "example  of text",
          "justification.  "
        ]
      },
      {
        "label": "single word line",
        "input": [
          [
            "What",
            "must",
            "be",
            "acknowledgment",
            "shall",
            "be"
          ],
          16
        ],
        "expected": [
          "What   must   be",
          "acknowledgment  ",
          "shall be        "
        ]
      },
      {
        "label": "last line left justified",
        "input": [
          [
            "a",
            "b",
            "c",
            "d",
            "e"
          ],
          3
        ],
        "expected": [
          "a b",
          "c d",
          "e  "
        ]
      },
      {
        "label": "one exact-width word",
        "input": [
          [
            "Longword"
          ],
          8
        ],
        "expected": [
          "Longword"
        ]
      }
    ],
    "paramNames": [
      "words",
      "maxWidth"
    ],
    "paramTypes": [
      "string[]",
      "integer"
    ],
    "returnType": "list<string>"
  },
  "valid-palindrome": {
    "kind": "function",
    "entry": "isPalindrome",
    "checker": "exact",
    "cases": [
      {
        "label": "punctuation and case",
        "input": [
          "A man, a plan, a canal: Panama"
        ],
        "expected": true
      },
      {
        "label": "letters mismatch",
        "input": [
          "race a car"
        ],
        "expected": false
      },
      {
        "label": "only spaces",
        "input": [
          " "
        ],
        "expected": true
      },
      {
        "label": "digits count",
        "input": [
          "0P"
        ],
        "expected": false
      },
      {
        "label": "mixed digits and punctuation",
        "input": [
          "1,2,1"
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "boolean"
  },
  "is-subsequence": {
    "kind": "function",
    "entry": "isSubsequence",
    "checker": "exact",
    "cases": [
      {
        "label": "ordered match",
        "input": [
          "abc",
          "ahbgdc"
        ],
        "expected": true
      },
      {
        "label": "missing middle",
        "input": [
          "axc",
          "ahbgdc"
        ],
        "expected": false
      },
      {
        "label": "empty s",
        "input": [
          "",
          "anything"
        ],
        "expected": true
      },
      {
        "label": "repeated characters require order",
        "input": [
          "aaa",
          "aa"
        ],
        "expected": false
      },
      {
        "label": "not substring",
        "input": [
          "ace",
          "abcde"
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "s",
      "t"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "boolean"
  },
  "container-with-most-water": {
    "kind": "function",
    "entry": "maxArea",
    "checker": "exact",
    "cases": [
      {
        "label": "wide best pair",
        "input": [
          [
            1,
            8,
            6,
            2,
            5,
            4,
            8,
            3,
            7
          ]
        ],
        "expected": 49
      },
      {
        "label": "two lines",
        "input": [
          [
            1,
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "shorter wall limits",
        "input": [
          [
            4,
            3,
            2,
            1,
            4
          ]
        ],
        "expected": 16
      },
      {
        "label": "interior best",
        "input": [
          [
            1,
            2,
            4,
            3
          ]
        ],
        "expected": 4
      },
      {
        "label": "zero height edge",
        "input": [
          [
            0,
            2,
            0,
            4
          ]
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "height"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "two-sum-ii-input-array-is-sorted": {
    "kind": "function",
    "entry": "twoSum",
    "checker": "exact",
    "cases": [
      {
        "label": "outer pair",
        "input": [
          [
            2,
            7,
            11,
            15
          ],
          9
        ],
        "expected": [
          1,
          2
        ]
      },
      {
        "label": "skip middle",
        "input": [
          [
            2,
            3,
            4
          ],
          6
        ],
        "expected": [
          1,
          3
        ]
      },
      {
        "label": "negative target",
        "input": [
          [
            -1,
            0
          ],
          -1
        ],
        "expected": [
          1,
          2
        ]
      },
      {
        "label": "duplicate values",
        "input": [
          [
            1,
            1,
            3,
            4
          ],
          2
        ],
        "expected": [
          1,
          2
        ]
      }
    ],
    "paramNames": [
      "numbers",
      "target"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer[]"
  },
  "3sum": {
    "kind": "function",
    "entry": "threeSum",
    "checker": "tripletSet",
    "cases": [
      {
        "label": "duplicates collapse",
        "input": [
          [
            -1,
            0,
            1,
            2,
            -1,
            -4
          ]
        ],
        "expected": [
          [
            -1,
            -1,
            2
          ],
          [
            -1,
            0,
            1
          ]
        ]
      },
      {
        "label": "no triplet",
        "input": [
          [
            0,
            1,
            1
          ]
        ],
        "expected": []
      },
      {
        "label": "all zeros once",
        "input": [
          [
            0,
            0,
            0
          ]
        ],
        "expected": [
          [
            0,
            0,
            0
          ]
        ]
      },
      {
        "label": "many duplicates",
        "input": [
          [
            -2,
            0,
            0,
            2,
            2
          ]
        ],
        "expected": [
          [
            -2,
            0,
            2
          ]
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "list<list<integer>>"
  },
  "happy-number": {
    "kind": "function",
    "entry": "isHappy",
    "checker": "exact",
    "cases": [
      {
        "label": "reaches one",
        "input": [
          19
        ],
        "expected": true
      },
      {
        "label": "cycle at four",
        "input": [
          2
        ],
        "expected": false
      },
      {
        "label": "already one",
        "input": [
          1
        ],
        "expected": true
      },
      {
        "label": "larger happy",
        "input": [
          100
        ],
        "expected": true
      },
      {
        "label": "larger cycle",
        "input": [
          116
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "boolean"
  },
  "longest-substring-without-repeating-characters": {
    "kind": "function",
    "entry": "lengthOfLongestSubstring",
    "checker": "exact",
    "cases": [
      {
        "label": "repeat after window",
        "input": [
          "abcabcbb"
        ],
        "expected": 3
      },
      {
        "label": "all same",
        "input": [
          "bbbbb"
        ],
        "expected": 1
      },
      {
        "label": "substring not subsequence",
        "input": [
          "pwwkew"
        ],
        "expected": 3
      },
      {
        "label": "left pointer must not move backward",
        "input": [
          "abba"
        ],
        "expected": 2
      },
      {
        "label": "empty string",
        "input": [
          ""
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "integer"
  },
  "minimum-window-substring": {
    "kind": "function",
    "entry": "minWindow",
    "checker": "exact",
    "cases": [
      {
        "label": "classic shrink",
        "input": [
          "ADOBECODEBANC",
          "ABC"
        ],
        "expected": "BANC"
      },
      {
        "label": "single character",
        "input": [
          "a",
          "a"
        ],
        "expected": "a"
      },
      {
        "label": "not enough multiplicity",
        "input": [
          "a",
          "aa"
        ],
        "expected": ""
      },
      {
        "label": "duplicate required",
        "input": [
          "aa",
          "aa"
        ],
        "expected": "aa"
      },
      {
        "label": "case-sensitive",
        "input": [
          "ab",
          "A"
        ],
        "expected": ""
      }
    ],
    "paramNames": [
      "s",
      "t"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "string"
  },
  "substring-with-concatenation-of-all-words": {
    "kind": "function",
    "entry": "findSubstring",
    "checker": "arrayBag",
    "cases": [
      {
        "label": "two words two positions",
        "input": [
          "barfoothefoobarman",
          [
            "foo",
            "bar"
          ]
        ],
        "expected": [
          0,
          9
        ]
      },
      {
        "label": "missing duplicate word",
        "input": [
          "wordgoodgoodgoodbestword",
          [
            "word",
            "good",
            "best",
            "word"
          ]
        ],
        "expected": []
      },
      {
        "label": "overlapping windows",
        "input": [
          "barfoofoobarthefoobarman",
          [
            "bar",
            "foo",
            "the"
          ]
        ],
        "expected": [
          6,
          9,
          12
        ]
      },
      {
        "label": "duplicate words",
        "input": [
          "wordgoodgoodgoodbestword",
          [
            "word",
            "good",
            "best",
            "good"
          ]
        ],
        "expected": [
          8
        ]
      }
    ],
    "paramNames": [
      "s",
      "words"
    ],
    "paramTypes": [
      "string",
      "string[]"
    ],
    "returnType": "list<integer>"
  },
  "minimum-size-subarray-sum": {
    "kind": "function",
    "entry": "minSubArrayLen",
    "checker": "exact",
    "cases": [
      {
        "label": "shrink to length two",
        "input": [
          7,
          [
            2,
            3,
            1,
            2,
            4,
            3
          ]
        ],
        "expected": 2
      },
      {
        "label": "single element enough",
        "input": [
          4,
          [
            1,
            4,
            4
          ]
        ],
        "expected": 1
      },
      {
        "label": "no qualifying window",
        "input": [
          11,
          [
            1,
            1,
            1,
            1,
            1,
            1,
            1,
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "entire array required",
        "input": [
          15,
          [
            1,
            2,
            3,
            4,
            5
          ]
        ],
        "expected": 5
      },
      {
        "label": "late short window",
        "input": [
          8,
          [
            1,
            1,
            1,
            1,
            8
          ]
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "target",
      "nums"
    ],
    "paramTypes": [
      "integer",
      "integer[]"
    ],
    "returnType": "integer"
  },
  "valid-sudoku": {
    "kind": "function",
    "entry": "isValidSudoku",
    "checker": "exact",
    "cases": [
      {
        "label": "valid partial board",
        "input": [
          [
            [
              "5",
              "3",
              ".",
              ".",
              "7",
              ".",
              ".",
              ".",
              "."
            ],
            [
              "6",
              ".",
              ".",
              "1",
              "9",
              "5",
              ".",
              ".",
              "."
            ],
            [
              ".",
              "9",
              "8",
              ".",
              ".",
              ".",
              ".",
              "6",
              "."
            ],
            [
              "8",
              ".",
              ".",
              ".",
              "6",
              ".",
              ".",
              ".",
              "3"
            ],
            [
              "4",
              ".",
              ".",
              "8",
              ".",
              "3",
              ".",
              ".",
              "1"
            ],
            [
              "7",
              ".",
              ".",
              ".",
              "2",
              ".",
              ".",
              ".",
              "6"
            ],
            [
              ".",
              "6",
              ".",
              ".",
              ".",
              ".",
              "2",
              "8",
              "."
            ],
            [
              ".",
              ".",
              ".",
              "4",
              "1",
              "9",
              ".",
              ".",
              "5"
            ],
            [
              ".",
              ".",
              ".",
              ".",
              "8",
              ".",
              ".",
              "7",
              "9"
            ]
          ]
        ],
        "expected": true
      },
      {
        "label": "duplicate in column",
        "input": [
          [
            [
              "8",
              "3",
              ".",
              ".",
              "7",
              ".",
              ".",
              ".",
              "."
            ],
            [
              "6",
              ".",
              ".",
              "1",
              "9",
              "5",
              ".",
              ".",
              "."
            ],
            [
              ".",
              "9",
              "8",
              ".",
              ".",
              ".",
              ".",
              "6",
              "."
            ],
            [
              "8",
              ".",
              ".",
              ".",
              "6",
              ".",
              ".",
              ".",
              "3"
            ],
            [
              "4",
              ".",
              ".",
              "8",
              ".",
              "3",
              ".",
              ".",
              "1"
            ],
            [
              "7",
              ".",
              ".",
              ".",
              "2",
              ".",
              ".",
              ".",
              "6"
            ],
            [
              ".",
              "6",
              ".",
              ".",
              ".",
              ".",
              "2",
              "8",
              "."
            ],
            [
              ".",
              ".",
              ".",
              "4",
              "1",
              "9",
              ".",
              ".",
              "5"
            ],
            [
              ".",
              ".",
              ".",
              ".",
              "8",
              ".",
              ".",
              "7",
              "9"
            ]
          ]
        ],
        "expected": false
      },
      {
        "label": "duplicate in row",
        "input": [
          [
            [
              "5",
              "3",
              ".",
              ".",
              "7",
              ".",
              ".",
              ".",
              "5"
            ],
            [
              "6",
              ".",
              ".",
              "1",
              "9",
              "5",
              ".",
              ".",
              "."
            ],
            [
              ".",
              "9",
              "8",
              ".",
              ".",
              ".",
              ".",
              "6",
              "."
            ],
            [
              "8",
              ".",
              ".",
              ".",
              "6",
              ".",
              ".",
              ".",
              "3"
            ],
            [
              "4",
              ".",
              ".",
              "8",
              ".",
              "3",
              ".",
              ".",
              "1"
            ],
            [
              "7",
              ".",
              ".",
              ".",
              "2",
              ".",
              ".",
              ".",
              "6"
            ],
            [
              ".",
              "6",
              ".",
              ".",
              ".",
              ".",
              "2",
              "8",
              "."
            ],
            [
              ".",
              ".",
              ".",
              "4",
              "1",
              "9",
              ".",
              ".",
              "5"
            ],
            [
              ".",
              ".",
              ".",
              ".",
              "8",
              ".",
              ".",
              "7",
              "9"
            ]
          ]
        ],
        "expected": false
      },
      {
        "label": "duplicate in box only",
        "input": [
          [
            [
              "5",
              "3",
              ".",
              ".",
              "7",
              ".",
              ".",
              ".",
              "."
            ],
            [
              "6",
              ".",
              ".",
              "1",
              "9",
              "5",
              ".",
              ".",
              "."
            ],
            [
              ".",
              "5",
              "8",
              ".",
              ".",
              ".",
              ".",
              "6",
              "."
            ],
            [
              "8",
              ".",
              ".",
              ".",
              "6",
              ".",
              ".",
              ".",
              "3"
            ],
            [
              "4",
              ".",
              ".",
              "8",
              ".",
              "3",
              ".",
              ".",
              "1"
            ],
            [
              "7",
              ".",
              ".",
              ".",
              "2",
              ".",
              ".",
              ".",
              "6"
            ],
            [
              ".",
              "6",
              ".",
              ".",
              ".",
              ".",
              "2",
              "8",
              "."
            ],
            [
              ".",
              ".",
              ".",
              "4",
              "1",
              "9",
              ".",
              ".",
              "5"
            ],
            [
              ".",
              ".",
              ".",
              ".",
              "8",
              ".",
              ".",
              "7",
              "9"
            ]
          ]
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "board"
    ],
    "paramTypes": [
      "character[][]"
    ],
    "returnType": "boolean"
  },
  "spiral-matrix": {
    "kind": "function",
    "entry": "spiralOrder",
    "checker": "exact",
    "cases": [
      {
        "label": "square matrix",
        "input": [
          [
            [
              1,
              2,
              3
            ],
            [
              4,
              5,
              6
            ],
            [
              7,
              8,
              9
            ]
          ]
        ],
        "expected": [
          1,
          2,
          3,
          6,
          9,
          8,
          7,
          4,
          5
        ]
      },
      {
        "label": "wide rectangle",
        "input": [
          [
            [
              1,
              2,
              3,
              4
            ],
            [
              5,
              6,
              7,
              8
            ],
            [
              9,
              10,
              11,
              12
            ]
          ]
        ],
        "expected": [
          1,
          2,
          3,
          4,
          8,
          12,
          11,
          10,
          9,
          5,
          6,
          7
        ]
      },
      {
        "label": "single row",
        "input": [
          [
            [
              1,
              2,
              3,
              4
            ]
          ]
        ],
        "expected": [
          1,
          2,
          3,
          4
        ]
      },
      {
        "label": "single column",
        "input": [
          [
            [
              1
            ],
            [
              2
            ],
            [
              3
            ]
          ]
        ],
        "expected": [
          1,
          2,
          3
        ]
      },
      {
        "label": "tall rectangle",
        "input": [
          [
            [
              1,
              2
            ],
            [
              3,
              4
            ],
            [
              5,
              6
            ],
            [
              7,
              8
            ]
          ]
        ],
        "expected": [
          1,
          2,
          4,
          6,
          8,
          7,
          5,
          3
        ]
      }
    ],
    "paramNames": [
      "matrix"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "list<integer>"
  },
  "rotate-image": {
    "kind": "function",
    "entry": "rotate",
    "checker": "exact",
    "outputParam": 0,
    "cases": [
      {
        "label": "odd square",
        "input": [
          [
            [
              1,
              2,
              3
            ],
            [
              4,
              5,
              6
            ],
            [
              7,
              8,
              9
            ]
          ]
        ],
        "expected": [
          [
            7,
            4,
            1
          ],
          [
            8,
            5,
            2
          ],
          [
            9,
            6,
            3
          ]
        ]
      },
      {
        "label": "even square",
        "input": [
          [
            [
              5,
              1,
              9,
              11
            ],
            [
              2,
              4,
              8,
              10
            ],
            [
              13,
              3,
              6,
              7
            ],
            [
              15,
              14,
              12,
              16
            ]
          ]
        ],
        "expected": [
          [
            15,
            13,
            2,
            5
          ],
          [
            14,
            3,
            4,
            1
          ],
          [
            12,
            6,
            8,
            9
          ],
          [
            16,
            7,
            10,
            11
          ]
        ]
      },
      {
        "label": "single cell",
        "input": [
          [
            [
              1
            ]
          ]
        ],
        "expected": [
          [
            1
          ]
        ]
      },
      {
        "label": "negative values",
        "input": [
          [
            [
              -1,
              -2
            ],
            [
              -3,
              -4
            ]
          ]
        ],
        "expected": [
          [
            -3,
            -1
          ],
          [
            -4,
            -2
          ]
        ]
      }
    ],
    "paramNames": [
      "matrix"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "void"
  },
  "set-matrix-zeroes": {
    "kind": "function",
    "entry": "setZeroes",
    "checker": "exact",
    "outputParam": 0,
    "cases": [
      {
        "label": "middle zero",
        "input": [
          [
            [
              1,
              1,
              1
            ],
            [
              1,
              0,
              1
            ],
            [
              1,
              1,
              1
            ]
          ]
        ],
        "expected": [
          [
            1,
            0,
            1
          ],
          [
            0,
            0,
            0
          ],
          [
            1,
            0,
            1
          ]
        ]
      },
      {
        "label": "first row and last column zero",
        "input": [
          [
            [
              0,
              1,
              2,
              0
            ],
            [
              3,
              4,
              5,
              2
            ],
            [
              1,
              3,
              1,
              5
            ]
          ]
        ],
        "expected": [
          [
            0,
            0,
            0,
            0
          ],
          [
            0,
            4,
            5,
            0
          ],
          [
            0,
            3,
            1,
            0
          ]
        ]
      },
      {
        "label": "single row zero",
        "input": [
          [
            [
              1,
              0,
              3
            ]
          ]
        ],
        "expected": [
          [
            0,
            0,
            0
          ]
        ]
      },
      {
        "label": "first column marker",
        "input": [
          [
            [
              1,
              2,
              3
            ],
            [
              0,
              5,
              6
            ],
            [
              7,
              8,
              9
            ]
          ]
        ],
        "expected": [
          [
            0,
            2,
            3
          ],
          [
            0,
            0,
            0
          ],
          [
            0,
            8,
            9
          ]
        ]
      },
      {
        "label": "no zero unchanged",
        "input": [
          [
            [
              1,
              2
            ],
            [
              3,
              4
            ]
          ]
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            3,
            4
          ]
        ]
      }
    ],
    "paramNames": [
      "matrix"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "void"
  },
  "game-of-life": {
    "kind": "function",
    "entry": "gameOfLife",
    "checker": "exact",
    "outputParam": 0,
    "cases": [
      {
        "label": "mixed board",
        "input": [
          [
            [
              0,
              1,
              0
            ],
            [
              0,
              0,
              1
            ],
            [
              1,
              1,
              1
            ],
            [
              0,
              0,
              0
            ]
          ]
        ],
        "expected": [
          [
            0,
            0,
            0
          ],
          [
            1,
            0,
            1
          ],
          [
            0,
            1,
            1
          ],
          [
            0,
            1,
            0
          ]
        ]
      },
      {
        "label": "birth in corner",
        "input": [
          [
            [
              1,
              1
            ],
            [
              1,
              0
            ]
          ]
        ],
        "expected": [
          [
            1,
            1
          ],
          [
            1,
            1
          ]
        ]
      },
      {
        "label": "single live cell dies",
        "input": [
          [
            [
              1
            ]
          ]
        ],
        "expected": [
          [
            0
          ]
        ]
      },
      {
        "label": "single dead cell stays dead",
        "input": [
          [
            [
              0
            ]
          ]
        ],
        "expected": [
          [
            0
          ]
        ]
      },
      {
        "label": "simultaneous blinker",
        "input": [
          [
            [
              0,
              1,
              0
            ],
            [
              0,
              1,
              0
            ],
            [
              0,
              1,
              0
            ]
          ]
        ],
        "expected": [
          [
            0,
            0,
            0
          ],
          [
            1,
            1,
            1
          ],
          [
            0,
            0,
            0
          ]
        ]
      }
    ],
    "paramNames": [
      "board"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "void"
  },
  "ransom-note": {
    "kind": "function",
    "entry": "canConstruct",
    "checker": "exact",
    "cases": [
      {
        "label": "missing letter",
        "input": [
          "a",
          "b"
        ],
        "expected": false
      },
      {
        "label": "insufficient multiplicity",
        "input": [
          "aa",
          "ab"
        ],
        "expected": false
      },
      {
        "label": "enough repeated letters",
        "input": [
          "aa",
          "aab"
        ],
        "expected": true
      },
      {
        "label": "uses all letters in any order",
        "input": [
          "aabb",
          "bbaa"
        ],
        "expected": true
      },
      {
        "label": "magazine too short despite matches",
        "input": [
          "abc",
          "ab"
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "ransomNote",
      "magazine"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "boolean"
  },
  "isomorphic-strings": {
    "kind": "function",
    "entry": "isIsomorphic",
    "checker": "exact",
    "cases": [
      {
        "label": "egg maps to add",
        "input": [
          "egg",
          "add"
        ],
        "expected": true
      },
      {
        "label": "inconsistent source mapping",
        "input": [
          "foo",
          "bar"
        ],
        "expected": false
      },
      {
        "label": "paper maps to title",
        "input": [
          "paper",
          "title"
        ],
        "expected": true
      },
      {
        "label": "two sources one target",
        "input": [
          "ab",
          "aa"
        ],
        "expected": false
      },
      {
        "label": "late target collision",
        "input": [
          "badc",
          "baba"
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "s",
      "t"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "boolean"
  },
  "word-pattern": {
    "kind": "function",
    "entry": "wordPattern",
    "checker": "exact",
    "cases": [
      {
        "label": "abba matches dog cat cat dog",
        "input": [
          "abba",
          "dog cat cat dog"
        ],
        "expected": true
      },
      {
        "label": "last word mismatch",
        "input": [
          "abba",
          "dog cat cat fish"
        ],
        "expected": false
      },
      {
        "label": "one pattern maps to many words",
        "input": [
          "aaaa",
          "dog cat cat dog"
        ],
        "expected": false
      },
      {
        "label": "many pattern letters share one word",
        "input": [
          "abba",
          "dog dog dog dog"
        ],
        "expected": false
      },
      {
        "label": "word count mismatch",
        "input": [
          "abba",
          "dog cat cat dog fish"
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "pattern",
      "s"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "boolean"
  },
  "valid-anagram": {
    "kind": "function",
    "entry": "isAnagram",
    "checker": "exact",
    "cases": [
      {
        "label": "letters reordered",
        "input": [
          "anagram",
          "nagaram"
        ],
        "expected": true
      },
      {
        "label": "different letters",
        "input": [
          "rat",
          "car"
        ],
        "expected": false
      },
      {
        "label": "length mismatch",
        "input": [
          "ab",
          "a"
        ],
        "expected": false
      },
      {
        "label": "same letters wrong multiplicity",
        "input": [
          "aacc",
          "ccac"
        ],
        "expected": false
      },
      {
        "label": "single repeated letter",
        "input": [
          "aaa",
          "aaa"
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "s",
      "t"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "boolean"
  },
  "group-anagrams": {
    "kind": "function",
    "entry": "groupAnagrams",
    "checker": "anagramGroups",
    "cases": [
      {
        "label": "mixed groups",
        "input": [
          [
            "eat",
            "tea",
            "tan",
            "ate",
            "nat",
            "bat"
          ]
        ],
        "expected": [
          [
            "eat",
            "tea",
            "ate"
          ],
          [
            "tan",
            "nat"
          ],
          [
            "bat"
          ]
        ]
      },
      {
        "label": "empty string group",
        "input": [
          [
            ""
          ]
        ],
        "expected": [
          [
            ""
          ]
        ]
      },
      {
        "label": "single string group",
        "input": [
          [
            "a"
          ]
        ],
        "expected": [
          [
            "a"
          ]
        ]
      },
      {
        "label": "duplicate words preserved",
        "input": [
          [
            "eat",
            "tea",
            "eat"
          ]
        ],
        "expected": [
          [
            "eat",
            "tea",
            "eat"
          ]
        ]
      },
      {
        "label": "separate non-anagram groups",
        "input": [
          [
            "abc",
            "bca",
            "foo",
            "ofo",
            "bar"
          ]
        ],
        "expected": [
          [
            "abc",
            "bca"
          ],
          [
            "foo",
            "ofo"
          ],
          [
            "bar"
          ]
        ]
      }
    ],
    "paramNames": [
      "strs"
    ],
    "paramTypes": [
      "string[]"
    ],
    "returnType": "list<list<string>>"
  },
  "contains-duplicate-ii": {
    "kind": "function",
    "entry": "containsNearbyDuplicate",
    "checker": "exact",
    "cases": [
      {
        "label": "duplicate within k",
        "input": [
          [
            1,
            2,
            3,
            1
          ],
          3
        ],
        "expected": true
      },
      {
        "label": "adjacent duplicate",
        "input": [
          [
            1,
            0,
            1,
            1
          ],
          1
        ],
        "expected": true
      },
      {
        "label": "duplicate beyond k",
        "input": [
          [
            1,
            2,
            3,
            1,
            2,
            3
          ],
          2
        ],
        "expected": false
      },
      {
        "label": "k zero cannot reuse index",
        "input": [
          [
            1,
            1
          ],
          0
        ],
        "expected": false
      },
      {
        "label": "negative duplicate within k",
        "input": [
          [
            -1,
            2,
            -1
          ],
          2
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "nums",
      "k"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "boolean"
  },
  "longest-consecutive-sequence": {
    "kind": "function",
    "entry": "longestConsecutive",
    "checker": "exact",
    "cases": [
      {
        "label": "unsorted run",
        "input": [
          [
            100,
            4,
            200,
            1,
            3,
            2
          ]
        ],
        "expected": 4
      },
      {
        "label": "duplicates in long run",
        "input": [
          [
            0,
            3,
            7,
            2,
            5,
            8,
            4,
            6,
            0,
            1
          ]
        ],
        "expected": 9
      },
      {
        "label": "duplicate should not extend",
        "input": [
          [
            1,
            0,
            1,
            2
          ]
        ],
        "expected": 3
      },
      {
        "label": "empty array",
        "input": [
          []
        ],
        "expected": 0
      },
      {
        "label": "negative run",
        "input": [
          [
            -2,
            -3,
            -1,
            5
          ]
        ],
        "expected": 3
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "summary-ranges": {
    "kind": "function",
    "entry": "summaryRanges",
    "checker": "exact",
    "cases": [
      {
        "label": "mixed ranges and singles",
        "input": [
          [
            0,
            1,
            2,
            4,
            5,
            7
          ]
        ],
        "expected": [
          "0->2",
          "4->5",
          "7"
        ]
      },
      {
        "label": "starts with single",
        "input": [
          [
            0,
            2,
            3,
            4,
            6,
            8,
            9
          ]
        ],
        "expected": [
          "0",
          "2->4",
          "6",
          "8->9"
        ]
      },
      {
        "label": "empty input",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "single value",
        "input": [
          [
            -1
          ]
        ],
        "expected": [
          "-1"
        ]
      },
      {
        "label": "negative to positive range",
        "input": [
          [
            -2,
            -1,
            0,
            2
          ]
        ],
        "expected": [
          "-2->0",
          "2"
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "list<string>"
  },
  "insert-interval": {
    "kind": "function",
    "entry": "insert",
    "checker": "exact",
    "cases": [
      {
        "label": "merge middle overlap",
        "input": [
          [
            [
              1,
              3
            ],
            [
              6,
              9
            ]
          ],
          [
            2,
            5
          ]
        ],
        "expected": [
          [
            1,
            5
          ],
          [
            6,
            9
          ]
        ]
      },
      {
        "label": "merge several intervals",
        "input": [
          [
            [
              1,
              2
            ],
            [
              3,
              5
            ],
            [
              6,
              7
            ],
            [
              8,
              10
            ],
            [
              12,
              16
            ]
          ],
          [
            4,
            8
          ]
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            3,
            10
          ],
          [
            12,
            16
          ]
        ]
      },
      {
        "label": "insert before all",
        "input": [
          [
            [
              3,
              5
            ]
          ],
          [
            1,
            2
          ]
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            3,
            5
          ]
        ]
      },
      {
        "label": "insert after all",
        "input": [
          [
            [
              1,
              2
            ]
          ],
          [
            3,
            5
          ]
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            3,
            5
          ]
        ]
      },
      {
        "label": "touching endpoints merge",
        "input": [
          [
            [
              1,
              2
            ],
            [
              5,
              6
            ]
          ],
          [
            2,
            5
          ]
        ],
        "expected": [
          [
            1,
            6
          ]
        ]
      }
    ],
    "paramNames": [
      "intervals",
      "newInterval"
    ],
    "paramTypes": [
      "integer[][]",
      "integer[]"
    ],
    "returnType": "integer[][]"
  },
  "minimum-number-of-arrows-to-burst-balloons": {
    "kind": "function",
    "entry": "findMinArrowShots",
    "checker": "exact",
    "cases": [
      {
        "label": "overlapping clusters",
        "input": [
          [
            [
              10,
              16
            ],
            [
              2,
              8
            ],
            [
              1,
              6
            ],
            [
              7,
              12
            ]
          ]
        ],
        "expected": 2
      },
      {
        "label": "no overlaps",
        "input": [
          [
            [
              1,
              2
            ],
            [
              3,
              4
            ],
            [
              5,
              6
            ],
            [
              7,
              8
            ]
          ]
        ],
        "expected": 4
      },
      {
        "label": "touching endpoints share arrow",
        "input": [
          [
            [
              1,
              2
            ],
            [
              2,
              3
            ],
            [
              3,
              4
            ],
            [
              4,
              5
            ]
          ]
        ],
        "expected": 2
      },
      {
        "label": "nested balloons",
        "input": [
          [
            [
              1,
              10
            ],
            [
              2,
              3
            ],
            [
              4,
              5
            ]
          ]
        ],
        "expected": 2
      },
      {
        "label": "negative coordinates",
        "input": [
          [
            [
              -3,
              -1
            ],
            [
              -2,
              2
            ],
            [
              3,
              4
            ]
          ]
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "points"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "integer"
  },
  "simplify-path": {
    "kind": "function",
    "entry": "simplifyPath",
    "checker": "exact",
    "cases": [
      {
        "label": "trailing slash removed",
        "input": [
          "/home/"
        ],
        "expected": "/home"
      },
      {
        "label": "repeated slashes collapsed",
        "input": [
          "/home//foo/"
        ],
        "expected": "/home/foo"
      },
      {
        "label": "parent from root stays root",
        "input": [
          "/../"
        ],
        "expected": "/"
      },
      {
        "label": "dot names are directories",
        "input": [
          "/.../a/../b/c/../d/./"
        ],
        "expected": "/.../b/d"
      },
      {
        "label": "multiple parent pops",
        "input": [
          "/a/./b/../../c/"
        ],
        "expected": "/c"
      }
    ],
    "paramNames": [
      "path"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "string"
  },
  "min-stack": {
    "kind": "class",
    "entry": "minStack",
    "className": "MinStack",
    "checker": "exact",
    "cases": [
      {
        "label": "classic negative minimum sequence",
        "input": [
          [
            "MinStack",
            "push",
            "push",
            "push",
            "getMin",
            "pop",
            "top",
            "getMin"
          ],
          [
            [],
            [
              -2
            ],
            [
              0
            ],
            [
              -3
            ],
            [],
            [],
            [],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          -3,
          null,
          0,
          -2
        ]
      },
      {
        "label": "duplicate minimum survives one pop",
        "input": [
          [
            "MinStack",
            "push",
            "push",
            "push",
            "getMin",
            "pop",
            "getMin",
            "pop",
            "getMin"
          ],
          [
            [],
            [
              2
            ],
            [
              1
            ],
            [
              1
            ],
            [],
            [],
            [],
            [],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          1,
          null,
          1,
          null,
          2
        ]
      },
      {
        "label": "top after decreasing pushes",
        "input": [
          [
            "MinStack",
            "push",
            "push",
            "top",
            "getMin",
            "pop",
            "top",
            "getMin"
          ],
          [
            [],
            [
              5
            ],
            [
              3
            ],
            [],
            [],
            [],
            [],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          3,
          3,
          null,
          5,
          5
        ]
      },
      {
        "label": "negative minimum after positive values",
        "input": [
          [
            "MinStack",
            "push",
            "push",
            "push",
            "getMin",
            "top"
          ],
          [
            [],
            [
              4
            ],
            [
              6
            ],
            [
              -1
            ],
            [],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          -1,
          -1
        ]
      }
    ]
  },
  "evaluate-reverse-polish-notation": {
    "kind": "function",
    "entry": "evalRPN",
    "checker": "exact",
    "cases": [
      {
        "label": "addition then multiply",
        "input": [
          [
            "2",
            "1",
            "+",
            "3",
            "*"
          ]
        ],
        "expected": 9
      },
      {
        "label": "division truncates toward zero",
        "input": [
          [
            "4",
            "13",
            "5",
            "/",
            "+"
          ]
        ],
        "expected": 6
      },
      {
        "label": "long mixed expression",
        "input": [
          [
            "10",
            "6",
            "9",
            "3",
            "+",
            "-11",
            "*",
            "/",
            "*",
            "17",
            "+",
            "5",
            "+"
          ]
        ],
        "expected": 22
      },
      {
        "label": "negative division truncates toward zero",
        "input": [
          [
            "-7",
            "3",
            "/"
          ]
        ],
        "expected": -2
      },
      {
        "label": "operand order matters for subtraction",
        "input": [
          [
            "5",
            "3",
            "-"
          ]
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "tokens"
    ],
    "paramTypes": [
      "string[]"
    ],
    "returnType": "integer"
  },
  "basic-calculator": {
    "kind": "function",
    "entry": "calculate",
    "checker": "exact",
    "cases": [
      {
        "label": "simple addition",
        "input": [
          "1 + 1"
        ],
        "expected": 2
      },
      {
        "label": "spaces and subtraction",
        "input": [
          " 2-1 + 2 "
        ],
        "expected": 3
      },
      {
        "label": "nested parentheses",
        "input": [
          "(1+(4+5+2)-3)+(6+8)"
        ],
        "expected": 23
      },
      {
        "label": "parenthesized subtraction",
        "input": [
          "2-(5-6)"
        ],
        "expected": 3
      },
      {
        "label": "nested subtraction",
        "input": [
          "1-(2-(3+4))"
        ],
        "expected": 6
      },
      {
        "label": "multi digit with nested signs",
        "input": [
          "10-(2+(3-4))"
        ],
        "expected": 9
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "integer"
  },
  "linked-list-cycle": {
    "kind": "function",
    "entry": "hasCycle",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      "cyclePos"
    ],
    "cyclePosParam": 1,
    "cases": [
      {
        "label": "cycle begins in middle",
        "input": [
          [
            3,
            2,
            0,
            -4
          ],
          1
        ],
        "expected": true
      },
      {
        "label": "cycle at head",
        "input": [
          [
            1,
            2
          ],
          0
        ],
        "expected": true
      },
      {
        "label": "single node no cycle",
        "input": [
          [
            1
          ],
          -1
        ],
        "expected": false
      },
      {
        "label": "empty list",
        "input": [
          [],
          -1
        ],
        "expected": false
      },
      {
        "label": "tail points to itself",
        "input": [
          [
            5,
            6,
            7
          ],
          2
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "head",
      "pos"
    ],
    "paramTypes": [
      "ListNode",
      "integer"
    ],
    "returnType": "boolean"
  },
  "add-two-numbers": {
    "kind": "function",
    "entry": "addTwoNumbers",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      "linkedList"
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "same length with carry",
        "input": [
          [
            2,
            4,
            3
          ],
          [
            5,
            6,
            4
          ]
        ],
        "expected": [
          7,
          0,
          8
        ]
      },
      {
        "label": "zero plus zero",
        "input": [
          [
            0
          ],
          [
            0
          ]
        ],
        "expected": [
          0
        ]
      },
      {
        "label": "carry extends result",
        "input": [
          [
            9,
            9,
            9,
            9,
            9,
            9,
            9
          ],
          [
            9,
            9,
            9,
            9
          ]
        ],
        "expected": [
          8,
          9,
          9,
          9,
          0,
          0,
          0,
          1
        ]
      },
      {
        "label": "different lengths",
        "input": [
          [
            1,
            8
          ],
          [
            0
          ]
        ],
        "expected": [
          1,
          8
        ]
      }
    ],
    "paramNames": [
      "l1",
      "l2"
    ],
    "paramTypes": [
      "ListNode",
      "ListNode"
    ],
    "returnType": "ListNode"
  },
  "merge-two-sorted-lists": {
    "kind": "function",
    "entry": "mergeTwoLists",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      "linkedList"
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "interleaved lists",
        "input": [
          [
            1,
            2,
            4
          ],
          [
            1,
            3,
            4
          ]
        ],
        "expected": [
          1,
          1,
          2,
          3,
          4,
          4
        ]
      },
      {
        "label": "both empty",
        "input": [
          [],
          []
        ],
        "expected": []
      },
      {
        "label": "left empty",
        "input": [
          [],
          [
            0
          ]
        ],
        "expected": [
          0
        ]
      },
      {
        "label": "negative values",
        "input": [
          [
            -3,
            -1,
            4
          ],
          [
            -2,
            2
          ]
        ],
        "expected": [
          -3,
          -2,
          -1,
          2,
          4
        ]
      }
    ],
    "paramNames": [
      "list1",
      "list2"
    ],
    "paramTypes": [
      "ListNode",
      "ListNode"
    ],
    "returnType": "ListNode"
  },
  "copy-list-with-random-pointer": {
    "kind": "function",
    "entry": "copyRandomList",
    "checker": "exact",
    "argTypes": [
      "randomList"
    ],
    "outputType": "randomList",
    "cases": [
      {
        "label": "mixed random pointers",
        "input": [
          [
            [
              7,
              null
            ],
            [
              13,
              0
            ],
            [
              11,
              4
            ],
            [
              10,
              2
            ],
            [
              1,
              0
            ]
          ]
        ],
        "expected": [
          [
            7,
            null
          ],
          [
            13,
            0
          ],
          [
            11,
            4
          ],
          [
            10,
            2
          ],
          [
            1,
            0
          ]
        ]
      },
      {
        "label": "self random pointer",
        "input": [
          [
            [
              1,
              0
            ]
          ]
        ],
        "expected": [
          [
            1,
            0
          ]
        ]
      },
      {
        "label": "duplicate values different randoms",
        "input": [
          [
            [
              3,
              null
            ],
            [
              3,
              0
            ],
            [
              3,
              null
            ]
          ]
        ],
        "expected": [
          [
            3,
            null
          ],
          [
            3,
            0
          ],
          [
            3,
            null
          ]
        ]
      },
      {
        "label": "empty random list",
        "input": [
          []
        ],
        "expected": []
      }
    ],
    "paramNames": [
      "head"
    ],
    "paramTypes": [
      "Node"
    ],
    "returnType": "Node"
  },
  "reverse-linked-list": {
    "kind": "function",
    "entry": "reverseList",
    "checker": "exact",
    "argTypes": [
      "linkedList"
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "five nodes",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ]
        ],
        "expected": [
          5,
          4,
          3,
          2,
          1
        ]
      },
      {
        "label": "two nodes",
        "input": [
          [
            1,
            2
          ]
        ],
        "expected": [
          2,
          1
        ]
      },
      {
        "label": "empty list",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "negative values",
        "input": [
          [
            -1,
            0,
            2
          ]
        ],
        "expected": [
          2,
          0,
          -1
        ]
      }
    ],
    "paramNames": [
      "head"
    ],
    "paramTypes": [
      "ListNode"
    ],
    "returnType": "ListNode"
  },
  "reverse-nodes-in-k-group": {
    "kind": "function",
    "entry": "reverseKGroup",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      null
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "pairs with leftover",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ],
          2
        ],
        "expected": [
          2,
          1,
          4,
          3,
          5
        ]
      },
      {
        "label": "triples with leftover",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ],
          3
        ],
        "expected": [
          3,
          2,
          1,
          4,
          5
        ]
      },
      {
        "label": "k equals one",
        "input": [
          [
            1,
            2,
            3
          ],
          1
        ],
        "expected": [
          1,
          2,
          3
        ]
      },
      {
        "label": "whole list group",
        "input": [
          [
            1,
            2,
            3,
            4
          ],
          4
        ],
        "expected": [
          4,
          3,
          2,
          1
        ]
      }
    ],
    "paramNames": [
      "head",
      "k"
    ],
    "paramTypes": [
      "ListNode",
      "integer"
    ],
    "returnType": "ListNode"
  },
  "remove-nth-node-from-end-of-list": {
    "kind": "function",
    "entry": "removeNthFromEnd",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      null
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "remove middle from end",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ],
          2
        ],
        "expected": [
          1,
          2,
          3,
          5
        ]
      },
      {
        "label": "remove only node",
        "input": [
          [
            1
          ],
          1
        ],
        "expected": []
      },
      {
        "label": "remove tail",
        "input": [
          [
            1,
            2
          ],
          1
        ],
        "expected": [
          1
        ]
      },
      {
        "label": "remove head",
        "input": [
          [
            1,
            2,
            3
          ],
          3
        ],
        "expected": [
          2,
          3
        ]
      }
    ],
    "paramNames": [
      "head",
      "n"
    ],
    "paramTypes": [
      "ListNode",
      "integer"
    ],
    "returnType": "ListNode"
  },
  "remove-duplicates-from-sorted-list-ii": {
    "kind": "function",
    "entry": "deleteDuplicates",
    "checker": "exact",
    "argTypes": [
      "linkedList"
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "middle duplicates removed",
        "input": [
          [
            1,
            2,
            3,
            3,
            4,
            4,
            5
          ]
        ],
        "expected": [
          1,
          2,
          5
        ]
      },
      {
        "label": "head duplicates removed",
        "input": [
          [
            1,
            1,
            1,
            2,
            3
          ]
        ],
        "expected": [
          2,
          3
        ]
      },
      {
        "label": "all values duplicated",
        "input": [
          [
            1,
            1,
            2,
            2
          ]
        ],
        "expected": []
      },
      {
        "label": "negative duplicates",
        "input": [
          [
            -2,
            -2,
            -1,
            0,
            0,
            1
          ]
        ],
        "expected": [
          -1,
          1
        ]
      }
    ],
    "paramNames": [
      "head"
    ],
    "paramTypes": [
      "ListNode"
    ],
    "returnType": "ListNode"
  },
  "rotate-list": {
    "kind": "function",
    "entry": "rotateRight",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      null
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "rotate by two",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ],
          2
        ],
        "expected": [
          4,
          5,
          1,
          2,
          3
        ]
      },
      {
        "label": "k larger than length",
        "input": [
          [
            0,
            1,
            2
          ],
          4
        ],
        "expected": [
          2,
          0,
          1
        ]
      },
      {
        "label": "k multiple of length",
        "input": [
          [
            1,
            2,
            3
          ],
          6
        ],
        "expected": [
          1,
          2,
          3
        ]
      },
      {
        "label": "empty list",
        "input": [
          [],
          3
        ],
        "expected": []
      }
    ],
    "paramNames": [
      "head",
      "k"
    ],
    "paramTypes": [
      "ListNode",
      "integer"
    ],
    "returnType": "ListNode"
  },
  "partition-list": {
    "kind": "function",
    "entry": "partition",
    "checker": "exact",
    "argTypes": [
      "linkedList",
      null
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "stable mixed partition",
        "input": [
          [
            1,
            4,
            3,
            2,
            5,
            2
          ],
          3
        ],
        "expected": [
          1,
          2,
          2,
          4,
          3,
          5
        ]
      },
      {
        "label": "head moves after smaller node",
        "input": [
          [
            2,
            1
          ],
          2
        ],
        "expected": [
          1,
          2
        ]
      },
      {
        "label": "all nodes stay before partition",
        "input": [
          [
            1,
            1,
            1
          ],
          5
        ],
        "expected": [
          1,
          1,
          1
        ]
      },
      {
        "label": "relative order preserved",
        "input": [
          [
            3,
            1,
            2,
            2,
            4
          ],
          3
        ],
        "expected": [
          1,
          2,
          2,
          3,
          4
        ]
      }
    ],
    "paramNames": [
      "head",
      "x"
    ],
    "paramTypes": [
      "ListNode",
      "integer"
    ],
    "returnType": "ListNode"
  },
  "maximum-depth-of-binary-tree": {
    "kind": "function",
    "entry": "maxDepth",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "balanced with leaves at depth three",
        "input": [
          [
            3,
            9,
            20,
            null,
            null,
            15,
            7
          ]
        ],
        "expected": 3
      },
      {
        "label": "right skew",
        "input": [
          [
            1,
            null,
            2
          ]
        ],
        "expected": 2
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": 0
      },
      {
        "label": "left skew depth four",
        "input": [
          [
            1,
            2,
            null,
            3,
            null,
            4
          ]
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "integer"
  },
  "same-tree": {
    "kind": "function",
    "entry": "isSameTree",
    "checker": "exact",
    "argTypes": [
      "binaryTree",
      "binaryTree"
    ],
    "cases": [
      {
        "label": "identical trees",
        "input": [
          [
            1,
            2,
            3
          ],
          [
            1,
            2,
            3
          ]
        ],
        "expected": true
      },
      {
        "label": "same values different null side",
        "input": [
          [
            1,
            2
          ],
          [
            1,
            null,
            2
          ]
        ],
        "expected": false
      },
      {
        "label": "same shape different values",
        "input": [
          [
            1,
            2,
            1
          ],
          [
            1,
            1,
            2
          ]
        ],
        "expected": false
      },
      {
        "label": "both empty",
        "input": [
          [],
          []
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "p",
      "q"
    ],
    "paramTypes": [
      "TreeNode",
      "TreeNode"
    ],
    "returnType": "boolean"
  },
  "invert-binary-tree": {
    "kind": "function",
    "entry": "invertTree",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "outputType": "binaryTree",
    "cases": [
      {
        "label": "full three level tree",
        "input": [
          [
            4,
            2,
            7,
            1,
            3,
            6,
            9
          ]
        ],
        "expected": [
          4,
          7,
          2,
          9,
          6,
          3,
          1
        ]
      },
      {
        "label": "three nodes",
        "input": [
          [
            2,
            1,
            3
          ]
        ],
        "expected": [
          2,
          3,
          1
        ]
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "sparse tree keeps null positions",
        "input": [
          [
            1,
            2,
            null,
            3
          ]
        ],
        "expected": [
          1,
          null,
          2,
          null,
          3
        ]
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "TreeNode"
  },
  "symmetric-tree": {
    "kind": "function",
    "entry": "isSymmetric",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "perfect mirror",
        "input": [
          [
            1,
            2,
            2,
            3,
            4,
            4,
            3
          ]
        ],
        "expected": true
      },
      {
        "label": "same values wrong sparse side",
        "input": [
          [
            1,
            2,
            2,
            null,
            3,
            null,
            3
          ]
        ],
        "expected": false
      },
      {
        "label": "single node",
        "input": [
          [
            1
          ]
        ],
        "expected": true
      },
      {
        "label": "cross sparse mirror",
        "input": [
          [
            1,
            2,
            2,
            null,
            3,
            3
          ]
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "boolean"
  },
  "construct-binary-tree-from-preorder-and-inorder-traversal": {
    "kind": "function",
    "entry": "buildTree",
    "checker": "exact",
    "outputType": "binaryTree",
    "cases": [
      {
        "label": "balanced right subtree",
        "input": [
          [
            3,
            9,
            20,
            15,
            7
          ],
          [
            9,
            3,
            15,
            20,
            7
          ]
        ],
        "expected": [
          3,
          9,
          20,
          null,
          null,
          15,
          7
        ]
      },
      {
        "label": "single negative node",
        "input": [
          [
            -1
          ],
          [
            -1
          ]
        ],
        "expected": [
          -1
        ]
      },
      {
        "label": "left skew",
        "input": [
          [
            1,
            2,
            3,
            4
          ],
          [
            4,
            3,
            2,
            1
          ]
        ],
        "expected": [
          1,
          2,
          null,
          3,
          null,
          4
        ]
      },
      {
        "label": "mixed child positions",
        "input": [
          [
            1,
            2,
            4,
            5,
            3,
            6
          ],
          [
            4,
            2,
            5,
            1,
            6,
            3
          ]
        ],
        "expected": [
          1,
          2,
          3,
          4,
          5,
          6
        ]
      }
    ],
    "paramNames": [
      "preorder",
      "inorder"
    ],
    "paramTypes": [
      "integer[]",
      "integer[]"
    ],
    "returnType": "TreeNode"
  },
  "construct-binary-tree-from-inorder-and-postorder-traversal": {
    "kind": "function",
    "entry": "buildTree",
    "checker": "exact",
    "outputType": "binaryTree",
    "cases": [
      {
        "label": "balanced right subtree",
        "input": [
          [
            9,
            3,
            15,
            20,
            7
          ],
          [
            9,
            15,
            7,
            20,
            3
          ]
        ],
        "expected": [
          3,
          9,
          20,
          null,
          null,
          15,
          7
        ]
      },
      {
        "label": "single negative node",
        "input": [
          [
            -1
          ],
          [
            -1
          ]
        ],
        "expected": [
          -1
        ]
      },
      {
        "label": "right skew",
        "input": [
          [
            1,
            2,
            3,
            4
          ],
          [
            4,
            3,
            2,
            1
          ]
        ],
        "expected": [
          1,
          null,
          2,
          null,
          3,
          null,
          4
        ]
      },
      {
        "label": "mixed child positions",
        "input": [
          [
            4,
            2,
            5,
            1,
            6,
            3
          ],
          [
            4,
            5,
            2,
            6,
            3,
            1
          ]
        ],
        "expected": [
          1,
          2,
          3,
          4,
          5,
          6
        ]
      }
    ],
    "paramNames": [
      "inorder",
      "postorder"
    ],
    "paramTypes": [
      "integer[]",
      "integer[]"
    ],
    "returnType": "TreeNode"
  },
  "populating-next-right-pointers-in-each-node-ii": {
    "kind": "function",
    "entry": "connect",
    "checker": "exact",
    "argTypes": [
      "nextTree"
    ],
    "outputType": "nextTree",
    "cases": [
      {
        "label": "sparse final level",
        "input": [
          [
            1,
            2,
            3,
            4,
            5,
            null,
            7
          ]
        ],
        "expected": [
          [
            1
          ],
          [
            2,
            3
          ],
          [
            4,
            5,
            7
          ]
        ]
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "single node",
        "input": [
          [
            1
          ]
        ],
        "expected": [
          [
            1
          ]
        ]
      },
      {
        "label": "missing middle children",
        "input": [
          [
            1,
            2,
            3,
            null,
            5,
            null,
            7
          ]
        ],
        "expected": [
          [
            1
          ],
          [
            2,
            3
          ],
          [
            5,
            7
          ]
        ]
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "Node"
    ],
    "returnType": "Node"
  },
  "flatten-binary-tree-to-linked-list": {
    "kind": "function",
    "entry": "flatten",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "outputParam": 0,
    "outputType": "binaryTree",
    "cases": [
      {
        "label": "branching preorder chain",
        "input": [
          [
            1,
            2,
            5,
            3,
            4,
            null,
            6
          ]
        ],
        "expected": [
          1,
          null,
          2,
          null,
          3,
          null,
          4,
          null,
          5,
          null,
          6
        ]
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "single zero node",
        "input": [
          [
            0
          ]
        ],
        "expected": [
          0
        ]
      },
      {
        "label": "already right chain",
        "input": [
          [
            1,
            null,
            2,
            null,
            3
          ]
        ],
        "expected": [
          1,
          null,
          2,
          null,
          3
        ]
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "void"
  },
  "path-sum": {
    "kind": "function",
    "entry": "hasPathSum",
    "checker": "exact",
    "argTypes": [
      "binaryTree",
      null
    ],
    "cases": [
      {
        "label": "target reaches leaf",
        "input": [
          [
            5,
            4,
            8,
            11,
            null,
            13,
            4,
            7,
            2,
            null,
            null,
            null,
            1
          ],
          22
        ],
        "expected": true
      },
      {
        "label": "no root to leaf target",
        "input": [
          [
            1,
            2,
            3
          ],
          5
        ],
        "expected": false
      },
      {
        "label": "empty tree",
        "input": [
          [],
          0
        ],
        "expected": false
      },
      {
        "label": "prefix sum is not enough",
        "input": [
          [
            1,
            2
          ],
          1
        ],
        "expected": false
      },
      {
        "label": "negative right path",
        "input": [
          [
            -2,
            null,
            -3
          ],
          -5
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "root",
      "targetSum"
    ],
    "paramTypes": [
      "TreeNode",
      "integer"
    ],
    "returnType": "boolean"
  },
  "sum-root-to-leaf-numbers": {
    "kind": "function",
    "entry": "sumNumbers",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "two leaves",
        "input": [
          [
            1,
            2,
            3
          ]
        ],
        "expected": 25
      },
      {
        "label": "zero digit leaf",
        "input": [
          [
            4,
            9,
            0,
            5,
            1
          ]
        ],
        "expected": 1026
      },
      {
        "label": "single zero",
        "input": [
          [
            0
          ]
        ],
        "expected": 0
      },
      {
        "label": "skewed digits",
        "input": [
          [
            1,
            null,
            2,
            null,
            3
          ]
        ],
        "expected": 123
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "integer"
  },
  "binary-tree-maximum-path-sum": {
    "kind": "function",
    "entry": "maxPathSum",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "through root",
        "input": [
          [
            1,
            2,
            3
          ]
        ],
        "expected": 6
      },
      {
        "label": "through inner node",
        "input": [
          [
            -10,
            9,
            20,
            null,
            null,
            15,
            7
          ]
        ],
        "expected": 42
      },
      {
        "label": "all negative picks one node",
        "input": [
          [
            -3,
            -2,
            -1
          ]
        ],
        "expected": -1
      },
      {
        "label": "one branch contribution",
        "input": [
          [
            2,
            -1,
            -2
          ]
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "integer"
  },
  "binary-search-tree-iterator": {
    "kind": "class",
    "className": "BSTIterator",
    "checker": "exact",
    "constructorArgTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "interleaved next and hasNext",
        "input": [
          [
            "BSTIterator",
            "next",
            "next",
            "hasNext",
            "next",
            "hasNext",
            "next",
            "hasNext",
            "next",
            "hasNext"
          ],
          [
            [
              [
                7,
                3,
                15,
                null,
                null,
                9,
                20
              ]
            ],
            [],
            [],
            [],
            [],
            [],
            [],
            [],
            [],
            []
          ]
        ],
        "expected": [
          null,
          3,
          7,
          true,
          9,
          true,
          15,
          true,
          20,
          false
        ]
      },
      {
        "label": "single node",
        "input": [
          [
            "BSTIterator",
            "hasNext",
            "next",
            "hasNext"
          ],
          [
            [
              [
                1
              ]
            ],
            [],
            [],
            []
          ]
        ],
        "expected": [
          null,
          true,
          1,
          false
        ]
      },
      {
        "label": "left skew bst",
        "input": [
          [
            "BSTIterator",
            "next",
            "next",
            "next",
            "hasNext"
          ],
          [
            [
              [
                3,
                2,
                null,
                1
              ]
            ],
            [],
            [],
            [],
            []
          ]
        ],
        "expected": [
          null,
          1,
          2,
          3,
          false
        ]
      }
    ]
  },
  "count-complete-tree-nodes": {
    "kind": "function",
    "entry": "countNodes",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "last level missing one",
        "input": [
          [
            1,
            2,
            3,
            4,
            5,
            6
          ]
        ],
        "expected": 6
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": 0
      },
      {
        "label": "single node",
        "input": [
          [
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "partial final level",
        "input": [
          [
            1,
            2,
            3,
            4
          ]
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "integer"
  },
  "lowest-common-ancestor-of-a-binary-tree": {
    "kind": "function",
    "entry": "lowestCommonAncestor",
    "checker": "exact",
    "argTypes": [
      "binaryTree",
      "treeNodeValue",
      "treeNodeValue"
    ],
    "outputType": "binaryTree",
    "cases": [
      {
        "label": "split at root",
        "input": [
          [
            3,
            5,
            1,
            6,
            2,
            0,
            8,
            null,
            null,
            7,
            4
          ],
          5,
          1
        ],
        "expected": [
          3,
          5,
          1,
          6,
          2,
          0,
          8,
          null,
          null,
          7,
          4
        ]
      },
      {
        "label": "ancestor is one target",
        "input": [
          [
            3,
            5,
            1,
            6,
            2,
            0,
            8,
            null,
            null,
            7,
            4
          ],
          5,
          4
        ],
        "expected": [
          5,
          6,
          2,
          null,
          null,
          7,
          4
        ]
      },
      {
        "label": "root is target",
        "input": [
          [
            1,
            2
          ],
          1,
          2
        ],
        "expected": [
          1,
          2
        ]
      },
      {
        "label": "sibling leaves",
        "input": [
          [
            3,
            5,
            1,
            6,
            2,
            0,
            8,
            null,
            null,
            7,
            4
          ],
          7,
          4
        ],
        "expected": [
          2,
          7,
          4
        ]
      }
    ],
    "paramNames": [
      "root",
      "p",
      "q"
    ],
    "paramTypes": [
      "TreeNode",
      "integer",
      "integer"
    ],
    "returnType": "TreeNode"
  },
  "binary-tree-right-side-view": {
    "kind": "function",
    "entry": "rightSideView",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "right branch with sparse leaves",
        "input": [
          [
            1,
            2,
            3,
            null,
            5,
            null,
            4
          ]
        ],
        "expected": [
          1,
          3,
          4
        ]
      },
      {
        "label": "left depth visible after right ends",
        "input": [
          [
            1,
            2,
            3,
            4,
            null,
            null,
            null,
            5
          ]
        ],
        "expected": [
          1,
          3,
          4,
          5
        ]
      },
      {
        "label": "right chain",
        "input": [
          [
            1,
            null,
            3
          ]
        ],
        "expected": [
          1,
          3
        ]
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": []
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "list<integer>"
  },
  "average-of-levels-in-binary-tree": {
    "kind": "function",
    "entry": "averageOfLevels",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "balanced with fractional middle",
        "input": [
          [
            3,
            9,
            20,
            null,
            null,
            15,
            7
          ]
        ],
        "expected": [
          3,
          14.5,
          11
        ]
      },
      {
        "label": "complete second example",
        "input": [
          [
            3,
            9,
            20,
            15,
            7
          ]
        ],
        "expected": [
          3,
          14.5,
          11
        ]
      },
      {
        "label": "single negative node",
        "input": [
          [
            -1
          ]
        ],
        "expected": [
          -1
        ]
      },
      {
        "label": "mixed signs average",
        "input": [
          [
            1,
            -2,
            3,
            4,
            5,
            -6,
            2
          ]
        ],
        "expected": [
          1,
          0.5,
          1.25
        ]
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "list<double>"
  },
  "binary-tree-level-order-traversal": {
    "kind": "function",
    "entry": "levelOrder",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "balanced with two leaves",
        "input": [
          [
            3,
            9,
            20,
            null,
            null,
            15,
            7
          ]
        ],
        "expected": [
          [
            3
          ],
          [
            9,
            20
          ],
          [
            15,
            7
          ]
        ]
      },
      {
        "label": "single node",
        "input": [
          [
            1
          ]
        ],
        "expected": [
          [
            1
          ]
        ]
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "sparse keeps left to right",
        "input": [
          [
            1,
            2,
            3,
            null,
            4,
            5
          ]
        ],
        "expected": [
          [
            1
          ],
          [
            2,
            3
          ],
          [
            4,
            5
          ]
        ]
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "list<list<integer>>"
  },
  "binary-tree-zigzag-level-order-traversal": {
    "kind": "function",
    "entry": "zigzagLevelOrder",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "balanced alternates middle",
        "input": [
          [
            3,
            9,
            20,
            null,
            null,
            15,
            7
          ]
        ],
        "expected": [
          [
            3
          ],
          [
            20,
            9
          ],
          [
            15,
            7
          ]
        ]
      },
      {
        "label": "single node",
        "input": [
          [
            1
          ]
        ],
        "expected": [
          [
            1
          ]
        ]
      },
      {
        "label": "empty tree",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "four levels alternate",
        "input": [
          [
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8
          ]
        ],
        "expected": [
          [
            1
          ],
          [
            3,
            2
          ],
          [
            4,
            5,
            6,
            7
          ],
          [
            8
          ]
        ]
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "list<list<integer>>"
  },
  "minimum-absolute-difference-in-bst": {
    "kind": "function",
    "entry": "getMinimumDifference",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "adjacent inorder leaves",
        "input": [
          [
            4,
            2,
            6,
            1,
            3
          ]
        ],
        "expected": 1
      },
      {
        "label": "gap hidden in right subtree",
        "input": [
          [
            1,
            0,
            48,
            null,
            null,
            12,
            49
          ]
        ],
        "expected": 1
      },
      {
        "label": "two nodes",
        "input": [
          [
            2,
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "minimum not parent child",
        "input": [
          [
            90,
            69,
            null,
            49,
            89,
            null,
            52
          ]
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "integer"
  },
  "kth-smallest-element-in-a-bst": {
    "kind": "function",
    "entry": "kthSmallest",
    "checker": "exact",
    "argTypes": [
      "binaryTree",
      null
    ],
    "cases": [
      {
        "label": "smallest with right child under left",
        "input": [
          [
            3,
            1,
            4,
            null,
            2
          ],
          1
        ],
        "expected": 1
      },
      {
        "label": "third smallest",
        "input": [
          [
            5,
            3,
            6,
            2,
            4,
            null,
            null,
            1
          ],
          3
        ],
        "expected": 3
      },
      {
        "label": "largest k",
        "input": [
          [
            3,
            1,
            4,
            null,
            2
          ],
          4
        ],
        "expected": 4
      },
      {
        "label": "left skew middle",
        "input": [
          [
            4,
            3,
            null,
            2,
            null,
            1
          ],
          2
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "root",
      "k"
    ],
    "paramTypes": [
      "TreeNode",
      "integer"
    ],
    "returnType": "integer"
  },
  "validate-binary-search-tree": {
    "kind": "function",
    "entry": "isValidBST",
    "checker": "exact",
    "argTypes": [
      "binaryTree"
    ],
    "cases": [
      {
        "label": "valid three nodes",
        "input": [
          [
            2,
            1,
            3
          ]
        ],
        "expected": true
      },
      {
        "label": "right child violates root",
        "input": [
          [
            5,
            1,
            4,
            null,
            null,
            3,
            6
          ]
        ],
        "expected": false
      },
      {
        "label": "duplicates are invalid",
        "input": [
          [
            2,
            2,
            2
          ]
        ],
        "expected": false
      },
      {
        "label": "deep descendant violates ancestor",
        "input": [
          [
            5,
            4,
            6,
            null,
            null,
            3,
            7
          ]
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "root"
    ],
    "paramTypes": [
      "TreeNode"
    ],
    "returnType": "boolean"
  },
  "merge-intervals": {
    "kind": "function",
    "entry": "merge",
    "checker": "exact",
    "cases": [
      {
        "label": "[[1,3],[2,6],[8,10],[15,18]]",
        "input": [
          [
            [
              1,
              3
            ],
            [
              2,
              6
            ],
            [
              8,
              10
            ],
            [
              15,
              18
            ]
          ]
        ],
        "expected": [
          [
            1,
            6
          ],
          [
            8,
            10
          ],
          [
            15,
            18
          ]
        ]
      },
      {
        "label": "touching endpoints: [[1,4],[4,5]]",
        "input": [
          [
            [
              1,
              4
            ],
            [
              4,
              5
            ]
          ]
        ],
        "expected": [
          [
            1,
            5
          ]
        ]
      },
      {
        "label": "unsorted input: [[5,6],[1,3],[2,4]]",
        "input": [
          [
            [
              5,
              6
            ],
            [
              1,
              3
            ],
            [
              2,
              4
            ]
          ]
        ],
        "expected": [
          [
            1,
            4
          ],
          [
            5,
            6
          ]
        ]
      },
      {
        "label": "single interval",
        "input": [
          [
            [
              1,
              4
            ]
          ]
        ],
        "expected": [
          [
            1,
            4
          ]
        ]
      },
      {
        "label": "contained interval: [[1,10],[2,3]]",
        "input": [
          [
            [
              1,
              10
            ],
            [
              2,
              3
            ]
          ]
        ],
        "expected": [
          [
            1,
            10
          ]
        ]
      }
    ],
    "paramNames": [
      "intervals"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "integer[][]"
  },
  "two-sum": {
    "kind": "function",
    "entry": "twoSum",
    "checker": "twoSum",
    "cases": [
      {
        "label": "nums=[2,7,11,15], target=9",
        "input": [
          [
            2,
            7,
            11,
            15
          ],
          9
        ],
        "expected": [
          0,
          1
        ]
      },
      {
        "label": "nums=[3,2,4], target=6",
        "input": [
          [
            3,
            2,
            4
          ],
          6
        ],
        "expected": [
          1,
          2
        ]
      },
      {
        "label": "duplicates: nums=[3,3], target=6",
        "input": [
          [
            3,
            3
          ],
          6
        ],
        "expected": [
          0,
          1
        ]
      },
      {
        "label": "negatives: nums=[-3,4,3,90], target=0",
        "input": [
          [
            -3,
            4,
            3,
            90
          ],
          0
        ],
        "expected": [
          0,
          2
        ]
      }
    ],
    "paramNames": [
      "nums",
      "target"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer[]"
  },
  "lru-cache": {
    "kind": "class",
    "entry": "lru",
    "className": "LRUCache",
    "checker": "exact",
    "cases": [
      {
        "label": "capacity 2, classic eviction sequence",
        "input": [
          [
            "LRUCache",
            "put",
            "put",
            "get",
            "put",
            "get",
            "put",
            "get",
            "get",
            "get"
          ],
          [
            [
              2
            ],
            [
              1,
              1
            ],
            [
              2,
              2
            ],
            [
              1
            ],
            [
              3,
              3
            ],
            [
              2
            ],
            [
              4,
              4
            ],
            [
              1
            ],
            [
              3
            ],
            [
              4
            ]
          ]
        ],
        "expected": [
          null,
          null,
          null,
          1,
          null,
          -1,
          null,
          -1,
          3,
          4
        ]
      },
      {
        "label": "capacity 1, immediate eviction",
        "input": [
          [
            "LRUCache",
            "put",
            "get",
            "put",
            "get",
            "get"
          ],
          [
            [
              1
            ],
            [
              2,
              1
            ],
            [
              2
            ],
            [
              3,
              2
            ],
            [
              2
            ],
            [
              3
            ]
          ]
        ],
        "expected": [
          null,
          null,
          1,
          null,
          -1,
          2
        ]
      },
      {
        "label": "put updates refresh recency",
        "input": [
          [
            "LRUCache",
            "put",
            "put",
            "put",
            "get",
            "get"
          ],
          [
            [
              2
            ],
            [
              1,
              1
            ],
            [
              2,
              2
            ],
            [
              1,
              10
            ],
            [
              1
            ],
            [
              2
            ]
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          10,
          2
        ]
      }
    ]
  },
  "number-of-islands": {
    "kind": "function",
    "entry": "numIslands",
    "checker": "exact",
    "cases": [
      {
        "label": "one big island",
        "input": [
          [
            [
              "1",
              "1",
              "1",
              "1",
              "0"
            ],
            [
              "1",
              "1",
              "0",
              "1",
              "0"
            ],
            [
              "1",
              "1",
              "0",
              "0",
              "0"
            ],
            [
              "0",
              "0",
              "0",
              "0",
              "0"
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "three islands",
        "input": [
          [
            [
              "1",
              "1",
              "0",
              "0",
              "0"
            ],
            [
              "1",
              "1",
              "0",
              "0",
              "0"
            ],
            [
              "0",
              "0",
              "1",
              "0",
              "0"
            ],
            [
              "0",
              "0",
              "0",
              "1",
              "1"
            ]
          ]
        ],
        "expected": 3
      },
      {
        "label": "single land cell",
        "input": [
          [
            [
              "1"
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "all water",
        "input": [
          [
            [
              "0",
              "0"
            ],
            [
              "0",
              "0"
            ]
          ]
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "grid"
    ],
    "paramTypes": [
      "character[][]"
    ],
    "returnType": "integer"
  },
  "surrounded-regions": {
    "kind": "function",
    "entry": "solve",
    "checker": "exact",
    "outputParam": 0,
    "cases": [
      {
        "label": "enclosed region flips",
        "input": [
          [
            [
              "X",
              "X",
              "X",
              "X"
            ],
            [
              "X",
              "O",
              "O",
              "X"
            ],
            [
              "X",
              "X",
              "O",
              "X"
            ],
            [
              "X",
              "O",
              "X",
              "X"
            ]
          ]
        ],
        "expected": [
          [
            "X",
            "X",
            "X",
            "X"
          ],
          [
            "X",
            "X",
            "X",
            "X"
          ],
          [
            "X",
            "X",
            "X",
            "X"
          ],
          [
            "X",
            "O",
            "X",
            "X"
          ]
        ]
      },
      {
        "label": "single cell",
        "input": [
          [
            [
              "X"
            ]
          ]
        ],
        "expected": [
          [
            "X"
          ]
        ]
      },
      {
        "label": "border connected region stays",
        "input": [
          [
            [
              "O",
              "O",
              "O"
            ],
            [
              "O",
              "X",
              "O"
            ],
            [
              "O",
              "O",
              "O"
            ]
          ]
        ],
        "expected": [
          [
            "O",
            "O",
            "O"
          ],
          [
            "O",
            "X",
            "O"
          ],
          [
            "O",
            "O",
            "O"
          ]
        ]
      },
      {
        "label": "interior island flips",
        "input": [
          [
            [
              "X",
              "X",
              "X"
            ],
            [
              "X",
              "O",
              "X"
            ],
            [
              "X",
              "X",
              "X"
            ]
          ]
        ],
        "expected": [
          [
            "X",
            "X",
            "X"
          ],
          [
            "X",
            "X",
            "X"
          ],
          [
            "X",
            "X",
            "X"
          ]
        ]
      }
    ],
    "paramNames": [
      "board"
    ],
    "paramTypes": [
      "character[][]"
    ],
    "returnType": "void"
  },
  "clone-graph": {
    "kind": "function",
    "entry": "cloneGraph",
    "checker": "exact",
    "argTypes": [
      "graphNode"
    ],
    "outputType": "graphNode",
    "cases": [
      {
        "label": "square graph",
        "input": [
          [
            [
              2,
              4
            ],
            [
              1,
              3
            ],
            [
              2,
              4
            ],
            [
              1,
              3
            ]
          ]
        ],
        "expected": [
          [
            2,
            4
          ],
          [
            1,
            3
          ],
          [
            2,
            4
          ],
          [
            1,
            3
          ]
        ]
      },
      {
        "label": "single isolated node",
        "input": [
          [
            []
          ]
        ],
        "expected": [
          []
        ]
      },
      {
        "label": "empty graph",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "chain graph deep copy",
        "input": [
          [
            [
              2
            ],
            [
              1,
              3
            ],
            [
              2
            ]
          ]
        ],
        "expected": [
          [
            2
          ],
          [
            1,
            3
          ],
          [
            2
          ]
        ]
      }
    ],
    "paramNames": [
      "edges"
    ],
    "paramTypes": [
      "Node"
    ],
    "returnType": "Node"
  },
  "evaluate-division": {
    "kind": "function",
    "entry": "calcEquation",
    "checker": "exact",
    "cases": [
      {
        "label": "connected and unknown queries",
        "input": [
          [
            [
              "a",
              "b"
            ],
            [
              "b",
              "c"
            ]
          ],
          [
            2,
            3
          ],
          [
            [
              "a",
              "c"
            ],
            [
              "b",
              "a"
            ],
            [
              "a",
              "e"
            ],
            [
              "a",
              "a"
            ],
            [
              "x",
              "x"
            ]
          ]
        ],
        "expected": [
          6,
          0.5,
          -1,
          1,
          -1
        ]
      },
      {
        "label": "multi component reciprocal",
        "input": [
          [
            [
              "a",
              "b"
            ],
            [
              "b",
              "c"
            ],
            [
              "bc",
              "cd"
            ]
          ],
          [
            1.5,
            2.5,
            5
          ],
          [
            [
              "a",
              "c"
            ],
            [
              "c",
              "b"
            ],
            [
              "bc",
              "cd"
            ],
            [
              "cd",
              "bc"
            ]
          ]
        ],
        "expected": [
          3.75,
          0.4,
          5,
          0.2
        ]
      },
      {
        "label": "single equation unknowns",
        "input": [
          [
            [
              "a",
              "b"
            ]
          ],
          [
            0.5
          ],
          [
            [
              "a",
              "b"
            ],
            [
              "b",
              "a"
            ],
            [
              "a",
              "c"
            ],
            [
              "x",
              "y"
            ]
          ]
        ],
        "expected": [
          0.5,
          2,
          -1,
          -1
        ]
      },
      {
        "label": "disconnected components",
        "input": [
          [
            [
              "a",
              "b"
            ],
            [
              "c",
              "d"
            ]
          ],
          [
            2,
            4
          ],
          [
            [
              "a",
              "d"
            ],
            [
              "c",
              "d"
            ],
            [
              "d",
              "c"
            ]
          ]
        ],
        "expected": [
          -1,
          4,
          0.25
        ]
      }
    ],
    "paramNames": [
      "equations",
      "values",
      "queries"
    ],
    "paramTypes": [
      "list<list<string>>",
      "double[]",
      "list<list<string>>"
    ],
    "returnType": "double[]"
  },
  "course-schedule": {
    "kind": "function",
    "entry": "canFinish",
    "checker": "exact",
    "cases": [
      {
        "label": "single prerequisite",
        "input": [
          2,
          [
            [
              1,
              0
            ]
          ]
        ],
        "expected": true
      },
      {
        "label": "two course cycle",
        "input": [
          2,
          [
            [
              1,
              0
            ],
            [
              0,
              1
            ]
          ]
        ],
        "expected": false
      },
      {
        "label": "disconnected courses",
        "input": [
          5,
          [
            [
              1,
              0
            ],
            [
              2,
              0
            ],
            [
              3,
              1
            ]
          ]
        ],
        "expected": true
      },
      {
        "label": "long cycle",
        "input": [
          4,
          [
            [
              1,
              0
            ],
            [
              2,
              1
            ],
            [
              3,
              2
            ],
            [
              1,
              3
            ]
          ]
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "numCourses",
      "prerequisites"
    ],
    "paramTypes": [
      "integer",
      "integer[][]"
    ],
    "returnType": "boolean"
  },
  "course-schedule-ii": {
    "kind": "function",
    "entry": "findOrder",
    "checker": "topologicalOrder",
    "cases": [
      {
        "label": "single prerequisite order",
        "input": [
          2,
          [
            [
              1,
              0
            ]
          ]
        ],
        "expected": [
          0,
          1
        ]
      },
      {
        "label": "branching prerequisites",
        "input": [
          4,
          [
            [
              1,
              0
            ],
            [
              2,
              0
            ],
            [
              3,
              1
            ],
            [
              3,
              2
            ]
          ]
        ],
        "expected": [
          0,
          1,
          2,
          3
        ]
      },
      {
        "label": "single course no prerequisites",
        "input": [
          1,
          []
        ],
        "expected": [
          0
        ]
      },
      {
        "label": "cycle returns empty",
        "input": [
          2,
          [
            [
              1,
              0
            ],
            [
              0,
              1
            ]
          ]
        ],
        "expected": []
      }
    ],
    "paramNames": [
      "numCourses",
      "prerequisites"
    ],
    "paramTypes": [
      "integer",
      "integer[][]"
    ],
    "returnType": "integer[]"
  },
  "snakes-and-ladders": {
    "kind": "function",
    "entry": "snakesAndLadders",
    "checker": "exact",
    "cases": [
      {
        "label": "example board four moves",
        "input": [
          [
            [
              -1,
              -1,
              -1,
              -1,
              -1,
              -1
            ],
            [
              -1,
              -1,
              -1,
              -1,
              -1,
              -1
            ],
            [
              -1,
              -1,
              -1,
              -1,
              -1,
              -1
            ],
            [
              -1,
              35,
              -1,
              -1,
              13,
              -1
            ],
            [
              -1,
              -1,
              -1,
              -1,
              -1,
              -1
            ],
            [
              -1,
              15,
              -1,
              -1,
              -1,
              -1
            ]
          ]
        ],
        "expected": 4
      },
      {
        "label": "ladder reaches finish",
        "input": [
          [
            [
              -1,
              -1
            ],
            [
              -1,
              3
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "unreachable trap",
        "input": [
          [
            [
              -1,
              1,
              1,
              1
            ],
            [
              -1,
              1,
              1,
              1
            ],
            [
              -1,
              1,
              1,
              1
            ],
            [
              -1,
              1,
              1,
              -1
            ]
          ]
        ],
        "expected": -1
      },
      {
        "label": "plain three by three",
        "input": [
          [
            [
              -1,
              -1,
              -1
            ],
            [
              -1,
              -1,
              -1
            ],
            [
              -1,
              -1,
              -1
            ]
          ]
        ],
        "expected": 2
      }
    ],
    "paramNames": [
      "board"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "integer"
  },
  "minimum-genetic-mutation": {
    "kind": "function",
    "entry": "minMutation",
    "checker": "exact",
    "cases": [
      {
        "label": "one mutation",
        "input": [
          "AACCGGTT",
          "AACCGGTA",
          [
            "AACCGGTA"
          ]
        ],
        "expected": 1
      },
      {
        "label": "two step mutation",
        "input": [
          "AACCGGTT",
          "AAACGGTA",
          [
            "AACCGGTA",
            "AACCGCTA",
            "AAACGGTA"
          ]
        ],
        "expected": 2
      },
      {
        "label": "end missing from bank",
        "input": [
          "AACCGGTT",
          "AACCGGTA",
          []
        ],
        "expected": -1
      },
      {
        "label": "three step chain",
        "input": [
          "AAAAACCC",
          "AACCCCCC",
          [
            "AAAACCCC",
            "AAACCCCC",
            "AACCCCCC"
          ]
        ],
        "expected": 3
      }
    ],
    "paramNames": [
      "startGene",
      "endGene",
      "bank"
    ],
    "paramTypes": [
      "string",
      "string",
      "string[]"
    ],
    "returnType": "integer"
  },
  "word-ladder": {
    "kind": "function",
    "entry": "ladderLength",
    "checker": "exact",
    "cases": [
      {
        "label": "classic ladder",
        "input": [
          "hit",
          "cog",
          [
            "hot",
            "dot",
            "dog",
            "lot",
            "log",
            "cog"
          ]
        ],
        "expected": 5
      },
      {
        "label": "end word absent",
        "input": [
          "hit",
          "cog",
          [
            "hot",
            "dot",
            "dog",
            "lot",
            "log"
          ]
        ],
        "expected": 0
      },
      {
        "label": "direct one letter change",
        "input": [
          "a",
          "c",
          [
            "a",
            "b",
            "c"
          ]
        ],
        "expected": 2
      },
      {
        "label": "shorter route through decoy",
        "input": [
          "hit",
          "cog",
          [
            "hot",
            "dot",
            "dog",
            "lot",
            "log",
            "cog",
            "hog"
          ]
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "beginWord",
      "endWord",
      "wordList"
    ],
    "paramTypes": [
      "string",
      "string",
      "list<string>"
    ],
    "returnType": "integer"
  },
  "implement-trie-prefix-tree": {
    "kind": "class",
    "className": "Trie",
    "checker": "exact",
    "cases": [
      {
        "label": "example operations",
        "input": [
          [
            "Trie",
            "insert",
            "search",
            "search",
            "startsWith",
            "insert",
            "search"
          ],
          [
            [],
            [
              "apple"
            ],
            [
              "apple"
            ],
            [
              "app"
            ],
            [
              "app"
            ],
            [
              "app"
            ],
            [
              "app"
            ]
          ]
        ],
        "expected": [
          null,
          null,
          true,
          false,
          true,
          null,
          true
        ]
      },
      {
        "label": "prefix is not word",
        "input": [
          [
            "Trie",
            "insert",
            "startsWith",
            "search",
            "insert",
            "search"
          ],
          [
            [],
            [
              "app"
            ],
            [
              "ap"
            ],
            [
              "apple"
            ],
            [
              "apple"
            ],
            [
              "apple"
            ]
          ]
        ],
        "expected": [
          null,
          null,
          true,
          false,
          null,
          true
        ]
      },
      {
        "label": "shared prefix words",
        "input": [
          [
            "Trie",
            "insert",
            "insert",
            "search",
            "search",
            "startsWith"
          ],
          [
            [],
            [
              "car"
            ],
            [
              "cat"
            ],
            [
              "car"
            ],
            [
              "cap"
            ],
            [
              "ca"
            ]
          ]
        ],
        "expected": [
          null,
          null,
          null,
          true,
          false,
          true
        ]
      },
      {
        "label": "missing prefix",
        "input": [
          [
            "Trie",
            "insert",
            "startsWith",
            "search"
          ],
          [
            [],
            [
              "dog"
            ],
            [
              "do"
            ],
            [
              "dogs"
            ]
          ]
        ],
        "expected": [
          null,
          null,
          true,
          false
        ]
      }
    ]
  },
  "design-add-and-search-words-data-structure": {
    "kind": "class",
    "className": "WordDictionary",
    "checker": "exact",
    "cases": [
      {
        "label": "example wildcard searches",
        "input": [
          [
            "WordDictionary",
            "addWord",
            "addWord",
            "addWord",
            "search",
            "search",
            "search",
            "search"
          ],
          [
            [],
            [
              "bad"
            ],
            [
              "dad"
            ],
            [
              "mad"
            ],
            [
              "pad"
            ],
            [
              "bad"
            ],
            [
              ".ad"
            ],
            [
              "b.."
            ]
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          false,
          true,
          true,
          true
        ]
      },
      {
        "label": "dot matches exactly one letter",
        "input": [
          [
            "WordDictionary",
            "addWord",
            "search",
            "search",
            "search"
          ],
          [
            [],
            [
              "at"
            ],
            [
              "."
            ],
            [
              "a."
            ],
            [
              ".."
            ]
          ]
        ],
        "expected": [
          null,
          null,
          false,
          true,
          true
        ]
      },
      {
        "label": "branching wildcard",
        "input": [
          [
            "WordDictionary",
            "addWord",
            "addWord",
            "search",
            "search",
            "search"
          ],
          [
            [],
            [
              "ran"
            ],
            [
              "rune"
            ],
            [
              "r.n"
            ],
            [
              "ru.e"
            ],
            [
              "r..e"
            ]
          ]
        ],
        "expected": [
          null,
          null,
          null,
          true,
          true,
          true
        ]
      },
      {
        "label": "missing length",
        "input": [
          [
            "WordDictionary",
            "addWord",
            "search",
            "search"
          ],
          [
            [],
            [
              "code"
            ],
            [
              "co.e"
            ],
            [
              "co..e"
            ]
          ]
        ],
        "expected": [
          null,
          null,
          true,
          false
        ]
      }
    ]
  },
  "word-search-ii": {
    "kind": "function",
    "entry": "findWords",
    "checker": "arrayBag",
    "cases": [
      {
        "label": "example board words",
        "input": [
          [
            [
              "o",
              "a",
              "a",
              "n"
            ],
            [
              "e",
              "t",
              "a",
              "e"
            ],
            [
              "i",
              "h",
              "k",
              "r"
            ],
            [
              "i",
              "f",
              "l",
              "v"
            ]
          ],
          [
            "oath",
            "pea",
            "eat",
            "rain"
          ]
        ],
        "expected": [
          "oath",
          "eat"
        ]
      },
      {
        "label": "cannot reuse cell",
        "input": [
          [
            [
              "a",
              "b"
            ],
            [
              "c",
              "d"
            ]
          ],
          [
            "abcb"
          ]
        ],
        "expected": []
      },
      {
        "label": "shared prefixes",
        "input": [
          [
            [
              "a",
              "b"
            ],
            [
              "c",
              "d"
            ]
          ],
          [
            "ab",
            "abc",
            "abd",
            "acd",
            "bd"
          ]
        ],
        "expected": [
          "ab",
          "abd",
          "acd",
          "bd"
        ]
      },
      {
        "label": "duplicate board paths return word once",
        "input": [
          [
            [
              "a",
              "a"
            ],
            [
              "a",
              "a"
            ]
          ],
          [
            "a",
            "aa",
            "aaa",
            "aaaa"
          ]
        ],
        "expected": [
          "a",
          "aa",
          "aaa",
          "aaaa"
        ]
      }
    ],
    "paramNames": [
      "board",
      "words"
    ],
    "paramTypes": [
      "character[][]",
      "string[]"
    ],
    "returnType": "list<string>"
  },
  "letter-combinations-of-a-phone-number": {
    "kind": "function",
    "entry": "letterCombinations",
    "checker": "arrayBag",
    "cases": [
      {
        "label": "two digits",
        "input": [
          "23"
        ],
        "expected": [
          "ad",
          "ae",
          "af",
          "bd",
          "be",
          "bf",
          "cd",
          "ce",
          "cf"
        ]
      },
      {
        "label": "single digit",
        "input": [
          "2"
        ],
        "expected": [
          "a",
          "b",
          "c"
        ]
      },
      {
        "label": "empty digits",
        "input": [
          ""
        ],
        "expected": []
      },
      {
        "label": "four choices digit",
        "input": [
          "79"
        ],
        "expected": [
          "pw",
          "px",
          "py",
          "pz",
          "qw",
          "qx",
          "qy",
          "qz",
          "rw",
          "rx",
          "ry",
          "rz",
          "sw",
          "sx",
          "sy",
          "sz"
        ]
      }
    ],
    "paramNames": [
      "digits"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "list<string>"
  },
  "combinations": {
    "kind": "function",
    "entry": "combine",
    "checker": "integerCombinations",
    "cases": [
      {
        "label": "six pairs",
        "input": [
          4,
          2
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            1,
            3
          ],
          [
            1,
            4
          ],
          [
            2,
            3
          ],
          [
            2,
            4
          ],
          [
            3,
            4
          ]
        ]
      },
      {
        "label": "single number",
        "input": [
          1,
          1
        ],
        "expected": [
          [
            1
          ]
        ]
      },
      {
        "label": "choose all numbers",
        "input": [
          3,
          3
        ],
        "expected": [
          [
            1,
            2,
            3
          ]
        ]
      },
      {
        "label": "singletons",
        "input": [
          3,
          1
        ],
        "expected": [
          [
            1
          ],
          [
            2
          ],
          [
            3
          ]
        ]
      }
    ],
    "paramNames": [
      "n",
      "k"
    ],
    "paramTypes": [
      "integer",
      "integer"
    ],
    "returnType": "list<list<integer>>"
  },
  "permutations": {
    "kind": "function",
    "entry": "permute",
    "checker": "integerRows",
    "cases": [
      {
        "label": "three values",
        "input": [
          [
            1,
            2,
            3
          ]
        ],
        "expected": [
          [
            1,
            2,
            3
          ],
          [
            1,
            3,
            2
          ],
          [
            2,
            1,
            3
          ],
          [
            2,
            3,
            1
          ],
          [
            3,
            1,
            2
          ],
          [
            3,
            2,
            1
          ]
        ]
      },
      {
        "label": "two values with zero",
        "input": [
          [
            0,
            1
          ]
        ],
        "expected": [
          [
            0,
            1
          ],
          [
            1,
            0
          ]
        ]
      },
      {
        "label": "single value",
        "input": [
          [
            1
          ]
        ],
        "expected": [
          [
            1
          ]
        ]
      },
      {
        "label": "negative value",
        "input": [
          [
            -1,
            2
          ]
        ],
        "expected": [
          [
            -1,
            2
          ],
          [
            2,
            -1
          ]
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "list<list<integer>>"
  },
  "combination-sum": {
    "kind": "function",
    "entry": "combinationSum",
    "checker": "integerCombinations",
    "cases": [
      {
        "label": "reuse smaller candidate",
        "input": [
          [
            2,
            3,
            6,
            7
          ],
          7
        ],
        "expected": [
          [
            2,
            2,
            3
          ],
          [
            7
          ]
        ]
      },
      {
        "label": "multiple combinations",
        "input": [
          [
            2,
            3,
            5
          ],
          8
        ],
        "expected": [
          [
            2,
            2,
            2,
            2
          ],
          [
            2,
            3,
            3
          ],
          [
            3,
            5
          ]
        ]
      },
      {
        "label": "no combination",
        "input": [
          [
            2
          ],
          1
        ],
        "expected": []
      },
      {
        "label": "unsorted candidates",
        "input": [
          [
            7,
            3,
            2
          ],
          7
        ],
        "expected": [
          [
            2,
            2,
            3
          ],
          [
            7
          ]
        ]
      }
    ],
    "paramNames": [
      "candidates",
      "target"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "list<list<integer>>"
  },
  "n-queens-ii": {
    "kind": "function",
    "entry": "totalNQueens",
    "checker": "exact",
    "cases": [
      {
        "label": "four queens",
        "input": [
          4
        ],
        "expected": 2
      },
      {
        "label": "single queen",
        "input": [
          1
        ],
        "expected": 1
      },
      {
        "label": "two queens impossible",
        "input": [
          2
        ],
        "expected": 0
      },
      {
        "label": "five queens",
        "input": [
          5
        ],
        "expected": 10
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "integer"
  },
  "generate-parentheses": {
    "kind": "function",
    "entry": "generateParenthesis",
    "checker": "arrayBag",
    "cases": [
      {
        "label": "three pairs",
        "input": [
          3
        ],
        "expected": [
          "((()))",
          "(()())",
          "(())()",
          "()(())",
          "()()()"
        ]
      },
      {
        "label": "single pair",
        "input": [
          1
        ],
        "expected": [
          "()"
        ]
      },
      {
        "label": "two pairs",
        "input": [
          2
        ],
        "expected": [
          "(())",
          "()()"
        ]
      },
      {
        "label": "four pairs",
        "input": [
          4
        ],
        "expected": [
          "(((())))",
          "((()()))",
          "((())())",
          "((()))()",
          "(()(()))",
          "(()()())",
          "(()())()",
          "(())(())",
          "(())()()",
          "()((()))",
          "()(()())",
          "()(())()",
          "()()(())",
          "()()()()"
        ]
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "list<string>"
  },
  "word-search": {
    "kind": "function",
    "entry": "exist",
    "checker": "exact",
    "cases": [
      {
        "label": "turning path exists",
        "input": [
          [
            [
              "A",
              "B",
              "C",
              "E"
            ],
            [
              "S",
              "F",
              "C",
              "S"
            ],
            [
              "A",
              "D",
              "E",
              "E"
            ]
          ],
          "ABCCED"
        ],
        "expected": true
      },
      {
        "label": "horizontal and vertical path",
        "input": [
          [
            [
              "A",
              "B",
              "C",
              "E"
            ],
            [
              "S",
              "F",
              "C",
              "S"
            ],
            [
              "A",
              "D",
              "E",
              "E"
            ]
          ],
          "SEE"
        ],
        "expected": true
      },
      {
        "label": "cannot reuse cell",
        "input": [
          [
            [
              "A",
              "B",
              "C",
              "E"
            ],
            [
              "S",
              "F",
              "C",
              "S"
            ],
            [
              "A",
              "D",
              "E",
              "E"
            ]
          ],
          "ABCB"
        ],
        "expected": false
      },
      {
        "label": "single cell mismatch",
        "input": [
          [
            [
              "A"
            ]
          ],
          "B"
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "board",
      "word"
    ],
    "paramTypes": [
      "character[][]",
      "string"
    ],
    "returnType": "boolean"
  },
  "convert-sorted-array-to-binary-search-tree": {
    "kind": "function",
    "entry": "sortedArrayToBST",
    "checker": "balancedBst",
    "outputType": "binaryTree",
    "cases": [
      {
        "label": "five values",
        "input": [
          [
            -10,
            -3,
            0,
            5,
            9
          ]
        ],
        "expected": [
          0,
          -3,
          9,
          -10,
          null,
          5
        ]
      },
      {
        "label": "two values allow either root",
        "input": [
          [
            1,
            3
          ]
        ],
        "expected": [
          1,
          null,
          3
        ]
      },
      {
        "label": "single value",
        "input": [
          [
            7
          ]
        ],
        "expected": [
          7
        ]
      },
      {
        "label": "four values stay balanced",
        "input": [
          [
            -4,
            -2,
            1,
            8
          ]
        ],
        "expected": [
          -2,
          -4,
          1,
          null,
          null,
          null,
          8
        ]
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "TreeNode"
  },
  "longest-palindromic-substring": {
    "kind": "function",
    "entry": "longestPalindrome",
    "checker": "palindrome",
    "cases": [
      {
        "label": "s=\"babad\"",
        "input": [
          "babad"
        ],
        "expected": "bab"
      },
      {
        "label": "s=\"cbbd\"",
        "input": [
          "cbbd"
        ],
        "expected": "bb"
      },
      {
        "label": "single char: s=\"a\"",
        "input": [
          "a"
        ],
        "expected": "a"
      },
      {
        "label": "no repeat: s=\"ac\"",
        "input": [
          "ac"
        ],
        "expected": "a"
      },
      {
        "label": "whole string: s=\"racecar\"",
        "input": [
          "racecar"
        ],
        "expected": "racecar"
      }
    ],
    "paramNames": [
      "s"
    ],
    "paramTypes": [
      "string"
    ],
    "returnType": "string"
  },
  "search-insert-position": {
    "kind": "function",
    "entry": "searchInsert",
    "checker": "exact",
    "cases": [
      {
        "label": "target present",
        "input": [
          [
            1,
            3,
            5,
            6
          ],
          5
        ],
        "expected": 2
      },
      {
        "label": "insert in middle",
        "input": [
          [
            1,
            3,
            5,
            6
          ],
          2
        ],
        "expected": 1
      },
      {
        "label": "append after all values",
        "input": [
          [
            1,
            3,
            5,
            6
          ],
          7
        ],
        "expected": 4
      },
      {
        "label": "prepend before all values",
        "input": [
          [
            1,
            3,
            5,
            6
          ],
          0
        ],
        "expected": 0
      },
      {
        "label": "single matching value",
        "input": [
          [
            1
          ],
          1
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "nums",
      "target"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer"
  },
  "plus-one": {
    "kind": "function",
    "entry": "plusOne",
    "checker": "exact",
    "cases": [
      {
        "label": "no carry",
        "input": [
          [
            1,
            2,
            3
          ]
        ],
        "expected": [
          1,
          2,
          4
        ]
      },
      {
        "label": "carry through final digit",
        "input": [
          [
            1,
            2,
            9
          ]
        ],
        "expected": [
          1,
          3,
          0
        ]
      },
      {
        "label": "all nines grow length",
        "input": [
          [
            9,
            9,
            9
          ]
        ],
        "expected": [
          1,
          0,
          0,
          0
        ]
      },
      {
        "label": "zero",
        "input": [
          [
            0
          ]
        ],
        "expected": [
          1
        ]
      },
      {
        "label": "longer no carry sample",
        "input": [
          [
            4,
            3,
            2,
            1
          ]
        ],
        "expected": [
          4,
          3,
          2,
          2
        ]
      }
    ],
    "paramNames": [
      "digits"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer[]"
  },
  "add-binary": {
    "kind": "function",
    "entry": "addBinary",
    "checker": "exact",
    "cases": [
      {
        "label": "sample carry",
        "input": [
          "11",
          "1"
        ],
        "expected": "100"
      },
      {
        "label": "sample multi digit",
        "input": [
          "1010",
          "1011"
        ],
        "expected": "10101"
      },
      {
        "label": "zero plus zero",
        "input": [
          "0",
          "0"
        ],
        "expected": "0"
      },
      {
        "label": "cascading carry",
        "input": [
          "1111",
          "1"
        ],
        "expected": "10000"
      },
      {
        "label": "unequal lengths",
        "input": [
          "1",
          "111"
        ],
        "expected": "1000"
      }
    ],
    "paramNames": [
      "a",
      "b"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "string"
  },
  "single-number": {
    "kind": "function",
    "entry": "singleNumber",
    "checker": "exact",
    "cases": [
      {
        "label": "sample short",
        "input": [
          [
            2,
            2,
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "sample mixed order",
        "input": [
          [
            4,
            1,
            2,
            1,
            2
          ]
        ],
        "expected": 4
      },
      {
        "label": "single element",
        "input": [
          [
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "negative unique value",
        "input": [
          [
            -7,
            3,
            3
          ]
        ],
        "expected": -7
      },
      {
        "label": "zero is paired",
        "input": [
          [
            0,
            8,
            0,
            5,
            5
          ]
        ],
        "expected": 8
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "palindrome-number": {
    "kind": "function",
    "entry": "isPalindrome",
    "checker": "exact",
    "cases": [
      {
        "label": "positive palindrome",
        "input": [
          121
        ],
        "expected": true
      },
      {
        "label": "negative number",
        "input": [
          -121
        ],
        "expected": false
      },
      {
        "label": "trailing zero",
        "input": [
          10
        ],
        "expected": false
      },
      {
        "label": "zero",
        "input": [
          0
        ],
        "expected": true
      },
      {
        "label": "even length palindrome",
        "input": [
          1221
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "x"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "boolean"
  },
  "climbing-stairs": {
    "kind": "function",
    "entry": "climbStairs",
    "checker": "exact",
    "cases": [
      {
        "label": "two steps",
        "input": [
          2
        ],
        "expected": 2
      },
      {
        "label": "three steps",
        "input": [
          3
        ],
        "expected": 3
      },
      {
        "label": "one step",
        "input": [
          1
        ],
        "expected": 1
      },
      {
        "label": "four steps",
        "input": [
          4
        ],
        "expected": 5
      },
      {
        "label": "larger dynamic case",
        "input": [
          10
        ],
        "expected": 89
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "integer"
  },
  "sqrtx": {
    "kind": "function",
    "entry": "mySqrt",
    "checker": "exact",
    "cases": [
      {
        "label": "perfect square",
        "input": [
          4
        ],
        "expected": 2
      },
      {
        "label": "round down",
        "input": [
          8
        ],
        "expected": 2
      },
      {
        "label": "zero",
        "input": [
          0
        ],
        "expected": 0
      },
      {
        "label": "one",
        "input": [
          1
        ],
        "expected": 1
      },
      {
        "label": "large overflow guard",
        "input": [
          2147395600
        ],
        "expected": 46340
      }
    ],
    "paramNames": [
      "x"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "integer"
  },
  "factorial-trailing-zeroes": {
    "kind": "function",
    "entry": "trailingZeroes",
    "checker": "exact",
    "cases": [
      {
        "label": "no factor of five",
        "input": [
          3
        ],
        "expected": 0
      },
      {
        "label": "first trailing zero",
        "input": [
          5
        ],
        "expected": 1
      },
      {
        "label": "zero factorial",
        "input": [
          0
        ],
        "expected": 0
      },
      {
        "label": "extra factor from twenty five",
        "input": [
          25
        ],
        "expected": 6
      },
      {
        "label": "multiple powers of five",
        "input": [
          100
        ],
        "expected": 24
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "integer"
  },
  "house-robber": {
    "kind": "function",
    "entry": "rob",
    "checker": "exact",
    "cases": [
      {
        "label": "sample alternating",
        "input": [
          [
            1,
            2,
            3,
            1
          ]
        ],
        "expected": 4
      },
      {
        "label": "sample with middle peak",
        "input": [
          [
            2,
            7,
            9,
            3,
            1
          ]
        ],
        "expected": 12
      },
      {
        "label": "single house",
        "input": [
          [
            5
          ]
        ],
        "expected": 5
      },
      {
        "label": "all zeroes",
        "input": [
          [
            0,
            0,
            0
          ]
        ],
        "expected": 0
      },
      {
        "label": "greedy local choice fails",
        "input": [
          [
            2,
            1,
            1,
            2
          ]
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "maximum-subarray": {
    "kind": "function",
    "entry": "maxSubArray",
    "checker": "exact",
    "cases": [
      {
        "label": "mixed sample",
        "input": [
          [
            -2,
            1,
            -3,
            4,
            -1,
            2,
            1,
            -5,
            4
          ]
        ],
        "expected": 6
      },
      {
        "label": "single element",
        "input": [
          [
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "all positive",
        "input": [
          [
            5,
            4,
            -1,
            7,
            8
          ]
        ],
        "expected": 23
      },
      {
        "label": "all negative",
        "input": [
          [
            -8,
            -3,
            -6
          ]
        ],
        "expected": -3
      },
      {
        "label": "must restart after loss",
        "input": [
          [
            -1,
            2,
            3,
            -10,
            4,
            5
          ]
        ],
        "expected": 9
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "coin-change": {
    "kind": "function",
    "entry": "coinChange",
    "checker": "exact",
    "cases": [
      {
        "label": "sample makes amount",
        "input": [
          [
            1,
            2,
            5
          ],
          11
        ],
        "expected": 3
      },
      {
        "label": "impossible amount",
        "input": [
          [
            2
          ],
          3
        ],
        "expected": -1
      },
      {
        "label": "zero amount",
        "input": [
          [
            1
          ],
          0
        ],
        "expected": 0
      },
      {
        "label": "greedy fails",
        "input": [
          [
            1,
            3,
            4
          ],
          6
        ],
        "expected": 2
      },
      {
        "label": "large coin ignored",
        "input": [
          [
            5,
            7
          ],
          1
        ],
        "expected": -1
      }
    ],
    "paramNames": [
      "coins",
      "amount"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer"
  },
  "longest-increasing-subsequence": {
    "kind": "function",
    "entry": "lengthOfLIS",
    "checker": "exact",
    "cases": [
      {
        "label": "classic sample",
        "input": [
          [
            10,
            9,
            2,
            5,
            3,
            7,
            101,
            18
          ]
        ],
        "expected": 4
      },
      {
        "label": "sample with duplicates",
        "input": [
          [
            0,
            1,
            0,
            3,
            2,
            3
          ]
        ],
        "expected": 4
      },
      {
        "label": "all equal",
        "input": [
          [
            7,
            7,
            7,
            7,
            7,
            7,
            7
          ]
        ],
        "expected": 1
      },
      {
        "label": "strictly decreasing",
        "input": [
          [
            5,
            4,
            3,
            2,
            1
          ]
        ],
        "expected": 1
      },
      {
        "label": "subsequence not substring",
        "input": [
          [
            4,
            10,
            4,
            3,
            8,
            9
          ]
        ],
        "expected": 3
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "search-a-2d-matrix": {
    "kind": "function",
    "entry": "searchMatrix",
    "checker": "exact",
    "cases": [
      {
        "label": "target present",
        "input": [
          [
            [
              1,
              3,
              5,
              7
            ],
            [
              10,
              11,
              16,
              20
            ],
            [
              23,
              30,
              34,
              60
            ]
          ],
          3
        ],
        "expected": true
      },
      {
        "label": "target absent between rows",
        "input": [
          [
            [
              1,
              3,
              5,
              7
            ],
            [
              10,
              11,
              16,
              20
            ],
            [
              23,
              30,
              34,
              60
            ]
          ],
          13
        ],
        "expected": false
      },
      {
        "label": "single cell present",
        "input": [
          [
            [
              1
            ]
          ],
          1
        ],
        "expected": true
      },
      {
        "label": "single row last value",
        "input": [
          [
            [
              1,
              2,
              3,
              4
            ]
          ],
          4
        ],
        "expected": true
      },
      {
        "label": "before first value",
        "input": [
          [
            [
              5,
              6
            ],
            [
              8,
              9
            ]
          ],
          4
        ],
        "expected": false
      }
    ],
    "paramNames": [
      "matrix",
      "target"
    ],
    "paramTypes": [
      "integer[][]",
      "integer"
    ],
    "returnType": "boolean"
  },
  "find-peak-element": {
    "kind": "function",
    "entry": "findPeakElement",
    "checker": "exact",
    "cases": [
      {
        "label": "interior peak",
        "input": [
          [
            1,
            2,
            3,
            1
          ]
        ],
        "expected": 2
      },
      {
        "label": "single value",
        "input": [
          [
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "left edge peak",
        "input": [
          [
            3,
            2,
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "right edge peak",
        "input": [
          [
            1,
            2,
            3
          ]
        ],
        "expected": 2
      },
      {
        "label": "negative interior peak",
        "input": [
          [
            -5,
            -2,
            -4
          ]
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "find-minimum-in-rotated-sorted-array": {
    "kind": "function",
    "entry": "findMin",
    "checker": "exact",
    "cases": [
      {
        "label": "rotated middle",
        "input": [
          [
            3,
            4,
            5,
            1,
            2
          ]
        ],
        "expected": 1
      },
      {
        "label": "rotated with zero",
        "input": [
          [
            4,
            5,
            6,
            7,
            0,
            1,
            2
          ]
        ],
        "expected": 0
      },
      {
        "label": "not rotated",
        "input": [
          [
            11,
            13,
            15,
            17
          ]
        ],
        "expected": 11
      },
      {
        "label": "single value",
        "input": [
          [
            2
          ]
        ],
        "expected": 2
      },
      {
        "label": "minimum near end",
        "input": [
          [
            5,
            6,
            7,
            1,
            2,
            3,
            4
          ]
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "minimum-path-sum": {
    "kind": "function",
    "entry": "minPathSum",
    "checker": "exact",
    "cases": [
      {
        "label": "sample grid",
        "input": [
          [
            [
              1,
              3,
              1
            ],
            [
              1,
              5,
              1
            ],
            [
              4,
              2,
              1
            ]
          ]
        ],
        "expected": 7
      },
      {
        "label": "two rows",
        "input": [
          [
            [
              1,
              2,
              3
            ],
            [
              4,
              5,
              6
            ]
          ]
        ],
        "expected": 12
      },
      {
        "label": "single cell",
        "input": [
          [
            [
              5
            ]
          ]
        ],
        "expected": 5
      },
      {
        "label": "single column",
        "input": [
          [
            [
              1
            ],
            [
              2
            ],
            [
              3
            ]
          ]
        ],
        "expected": 6
      },
      {
        "label": "greedy path fails",
        "input": [
          [
            [
              1,
              2,
              100
            ],
            [
              2,
              100,
              1
            ],
            [
              1,
              1,
              1
            ]
          ]
        ],
        "expected": 6
      }
    ],
    "paramNames": [
      "grid"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "integer"
  },
  "unique-paths-ii": {
    "kind": "function",
    "entry": "uniquePathsWithObstacles",
    "checker": "exact",
    "cases": [
      {
        "label": "center obstacle",
        "input": [
          [
            [
              0,
              0,
              0
            ],
            [
              0,
              1,
              0
            ],
            [
              0,
              0,
              0
            ]
          ]
        ],
        "expected": 2
      },
      {
        "label": "small detour",
        "input": [
          [
            [
              0,
              1
            ],
            [
              0,
              0
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "start blocked",
        "input": [
          [
            [
              1
            ]
          ]
        ],
        "expected": 0
      },
      {
        "label": "finish blocked",
        "input": [
          [
            [
              0,
              0
            ],
            [
              0,
              1
            ]
          ]
        ],
        "expected": 0
      },
      {
        "label": "single row blocked path",
        "input": [
          [
            [
              0,
              1,
              0
            ]
          ]
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "obstacleGrid"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "integer"
  },
  "word-break": {
    "kind": "function",
    "entry": "wordBreak",
    "checker": "exact",
    "cases": [
      {
        "label": "leetcode sample",
        "input": [
          "leetcode",
          [
            "leet",
            "code"
          ]
        ],
        "expected": true
      },
      {
        "label": "reuse words",
        "input": [
          "applepenapple",
          [
            "apple",
            "pen"
          ]
        ],
        "expected": true
      },
      {
        "label": "missing middle segment",
        "input": [
          "catsandog",
          [
            "cats",
            "dog",
            "sand",
            "and",
            "cat"
          ]
        ],
        "expected": false
      },
      {
        "label": "single word",
        "input": [
          "a",
          [
            "a"
          ]
        ],
        "expected": true
      },
      {
        "label": "prefix greedy fails",
        "input": [
          "cars",
          [
            "car",
            "ca",
            "rs"
          ]
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "s",
      "wordDict"
    ],
    "paramTypes": [
      "string",
      "list<string>"
    ],
    "returnType": "boolean"
  },
  "number-of-1-bits": {
    "kind": "function",
    "entry": "hammingWeight",
    "checker": "exact",
    "cases": [
      {
        "label": "three set bits",
        "input": [
          11
        ],
        "expected": 3
      },
      {
        "label": "single high bit",
        "input": [
          128
        ],
        "expected": 1
      },
      {
        "label": "many set bits",
        "input": [
          2147483645
        ],
        "expected": 30
      },
      {
        "label": "one",
        "input": [
          1
        ],
        "expected": 1
      },
      {
        "label": "alternating bits",
        "input": [
          85
        ],
        "expected": 4
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "integer"
  },
  "single-number-ii": {
    "kind": "function",
    "entry": "singleNumber",
    "checker": "exact",
    "cases": [
      {
        "label": "short sample",
        "input": [
          [
            2,
            2,
            3,
            2
          ]
        ],
        "expected": 3
      },
      {
        "label": "zero and positive sample",
        "input": [
          [
            0,
            1,
            0,
            1,
            0,
            1,
            99
          ]
        ],
        "expected": 99
      },
      {
        "label": "single element",
        "input": [
          [
            7
          ]
        ],
        "expected": 7
      },
      {
        "label": "negative unique value",
        "input": [
          [
            -4,
            6,
            6,
            6
          ]
        ],
        "expected": -4
      },
      {
        "label": "xor pair logic fails",
        "input": [
          [
            5,
            5,
            5,
            8,
            8,
            8,
            11
          ]
        ],
        "expected": 11
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "bitwise-and-of-numbers-range": {
    "kind": "function",
    "entry": "rangeBitwiseAnd",
    "checker": "exact",
    "cases": [
      {
        "label": "sample range",
        "input": [
          5,
          7
        ],
        "expected": 4
      },
      {
        "label": "zero singleton",
        "input": [
          0,
          0
        ],
        "expected": 0
      },
      {
        "label": "large span clears all bits",
        "input": [
          1,
          2147483647
        ],
        "expected": 0
      },
      {
        "label": "same nonzero value",
        "input": [
          12,
          12
        ],
        "expected": 12
      },
      {
        "label": "shared prefix remains",
        "input": [
          24,
          31
        ],
        "expected": 24
      }
    ],
    "paramNames": [
      "left",
      "right"
    ],
    "paramTypes": [
      "integer",
      "integer"
    ],
    "returnType": "integer"
  },
  "triangle": {
    "kind": "function",
    "entry": "minimumTotal",
    "checker": "exact",
    "cases": [
      {
        "label": "sample triangle",
        "input": [
          [
            [
              2
            ],
            [
              3,
              4
            ],
            [
              6,
              5,
              7
            ],
            [
              4,
              1,
              8,
              3
            ]
          ]
        ],
        "expected": 11
      },
      {
        "label": "single negative value",
        "input": [
          [
            [
              -10
            ]
          ]
        ],
        "expected": -10
      },
      {
        "label": "two rows",
        "input": [
          [
            [
              1
            ],
            [
              2,
              3
            ]
          ]
        ],
        "expected": 3
      },
      {
        "label": "greedy child fails",
        "input": [
          [
            [
              1
            ],
            [
              2,
              100
            ],
            [
              100,
              1,
              1
            ]
          ]
        ],
        "expected": 4
      },
      {
        "label": "negative path improves",
        "input": [
          [
            [
              2
            ],
            [
              -3,
              4
            ],
            [
              6,
              -5,
              7
            ]
          ]
        ],
        "expected": -6
      }
    ],
    "paramNames": [
      "triangle"
    ],
    "paramTypes": [
      "list<list<integer>>"
    ],
    "returnType": "integer"
  },
  "edit-distance": {
    "kind": "function",
    "entry": "minDistance",
    "checker": "exact",
    "cases": [
      {
        "label": "horse to ros",
        "input": [
          "horse",
          "ros"
        ],
        "expected": 3
      },
      {
        "label": "intention to execution",
        "input": [
          "intention",
          "execution"
        ],
        "expected": 5
      },
      {
        "label": "empty source",
        "input": [
          "",
          "abc"
        ],
        "expected": 3
      },
      {
        "label": "same word",
        "input": [
          "same",
          "same"
        ],
        "expected": 0
      },
      {
        "label": "replacement beats delete insert",
        "input": [
          "abc",
          "adc"
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "word1",
      "word2"
    ],
    "paramTypes": [
      "string",
      "string"
    ],
    "returnType": "integer"
  },
  "maximal-square": {
    "kind": "function",
    "entry": "maximalSquare",
    "checker": "exact",
    "cases": [
      {
        "label": "sample square area four",
        "input": [
          [
            [
              "1",
              "0",
              "1",
              "0",
              "0"
            ],
            [
              "1",
              "0",
              "1",
              "1",
              "1"
            ],
            [
              "1",
              "1",
              "1",
              "1",
              "1"
            ],
            [
              "1",
              "0",
              "0",
              "1",
              "0"
            ]
          ]
        ],
        "expected": 4
      },
      {
        "label": "diagonal ones",
        "input": [
          [
            [
              "0",
              "1"
            ],
            [
              "1",
              "0"
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "single zero",
        "input": [
          [
            [
              "0"
            ]
          ]
        ],
        "expected": 0
      },
      {
        "label": "single one",
        "input": [
          [
            [
              "1"
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "area not side length",
        "input": [
          [
            [
              "1",
              "1",
              "1"
            ],
            [
              "1",
              "1",
              "1"
            ],
            [
              "1",
              "1",
              "1"
            ]
          ]
        ],
        "expected": 9
      }
    ],
    "paramNames": [
      "matrix"
    ],
    "paramTypes": [
      "character[][]"
    ],
    "returnType": "integer"
  },
  "maximum-sum-circular-subarray": {
    "kind": "function",
    "entry": "maxSubarraySumCircular",
    "checker": "exact",
    "cases": [
      {
        "label": "non-wrapping sample",
        "input": [
          [
            1,
            -2,
            3,
            -2
          ]
        ],
        "expected": 3
      },
      {
        "label": "wrapping sample",
        "input": [
          [
            5,
            -3,
            5
          ]
        ],
        "expected": 10
      },
      {
        "label": "all negative",
        "input": [
          [
            -3,
            -2,
            -3
          ]
        ],
        "expected": -2
      },
      {
        "label": "single value",
        "input": [
          [
            7
          ]
        ],
        "expected": 7
      },
      {
        "label": "wrap beats middle",
        "input": [
          [
            8,
            -1,
            -3,
            8
          ]
        ],
        "expected": 16
      }
    ],
    "paramNames": [
      "nums"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "search-in-rotated-sorted-array": {
    "kind": "function",
    "entry": "search",
    "checker": "exact",
    "cases": [
      {
        "label": "target in rotated suffix",
        "input": [
          [
            4,
            5,
            6,
            7,
            0,
            1,
            2
          ],
          0
        ],
        "expected": 4
      },
      {
        "label": "target absent",
        "input": [
          [
            4,
            5,
            6,
            7,
            0,
            1,
            2
          ],
          3
        ],
        "expected": -1
      },
      {
        "label": "single absent",
        "input": [
          [
            1
          ],
          0
        ],
        "expected": -1
      },
      {
        "label": "not rotated",
        "input": [
          [
            1,
            2,
            3,
            4
          ],
          3
        ],
        "expected": 2
      },
      {
        "label": "target in rotated prefix",
        "input": [
          [
            6,
            7,
            1,
            2,
            3,
            4,
            5
          ],
          7
        ],
        "expected": 1
      }
    ],
    "paramNames": [
      "nums",
      "target"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer"
  },
  "kth-largest-element-in-an-array": {
    "kind": "function",
    "entry": "findKthLargest",
    "checker": "exact",
    "cases": [
      {
        "label": "sample second largest",
        "input": [
          [
            3,
            2,
            1,
            5,
            6,
            4
          ],
          2
        ],
        "expected": 5
      },
      {
        "label": "duplicates count",
        "input": [
          [
            3,
            2,
            3,
            1,
            2,
            4,
            5,
            5,
            6
          ],
          4
        ],
        "expected": 4
      },
      {
        "label": "single value",
        "input": [
          [
            1
          ],
          1
        ],
        "expected": 1
      },
      {
        "label": "k is length",
        "input": [
          [
            2,
            1
          ],
          2
        ],
        "expected": 1
      },
      {
        "label": "negative values",
        "input": [
          [
            -1,
            -1,
            -2,
            -3
          ],
          2
        ],
        "expected": -1
      }
    ],
    "paramNames": [
      "nums",
      "k"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer"
  },
  "find-first-and-last-position-of-element-in-sorted-array": {
    "kind": "function",
    "entry": "searchRange",
    "checker": "exact",
    "cases": [
      {
        "label": "target appears twice",
        "input": [
          [
            5,
            7,
            7,
            8,
            8,
            10
          ],
          8
        ],
        "expected": [
          3,
          4
        ]
      },
      {
        "label": "target absent",
        "input": [
          [
            5,
            7,
            7,
            8,
            8,
            10
          ],
          6
        ],
        "expected": [
          -1,
          -1
        ]
      },
      {
        "label": "empty array",
        "input": [
          [],
          0
        ],
        "expected": [
          -1,
          -1
        ]
      },
      {
        "label": "all values match",
        "input": [
          [
            2,
            2,
            2
          ],
          2
        ],
        "expected": [
          0,
          2
        ]
      },
      {
        "label": "single match at end",
        "input": [
          [
            1,
            2,
            3,
            4
          ],
          4
        ],
        "expected": [
          3,
          3
        ]
      }
    ],
    "paramNames": [
      "nums",
      "target"
    ],
    "paramTypes": [
      "integer[]",
      "integer"
    ],
    "returnType": "integer[]"
  },
  "powx-n": {
    "kind": "function",
    "entry": "myPow",
    "checker": "approxNumber",
    "cases": [
      {
        "label": "positive exponent",
        "input": [
          2.0,
          10
        ],
        "expected": 1024.0
      },
      {
        "label": "decimal base",
        "input": [
          2.1,
          3
        ],
        "expected": 9.261
      },
      {
        "label": "negative exponent",
        "input": [
          2.0,
          -2
        ],
        "expected": 0.25
      },
      {
        "label": "zero exponent",
        "input": [
          5.5,
          0
        ],
        "expected": 1.0
      },
      {
        "label": "minimum integer exponent reciprocal path",
        "input": [
          1.0,
          -2147483648
        ],
        "expected": 1.0
      }
    ],
    "paramNames": [
      "x",
      "n"
    ],
    "paramTypes": [
      "double",
      "integer"
    ],
    "returnType": "double"
  },
  "interleaving-string": {
    "kind": "function",
    "entry": "isInterleave",
    "checker": "exact",
    "cases": [
      {
        "label": "sample true",
        "input": [
          "aabcc",
          "dbbca",
          "aadbbcbcac"
        ],
        "expected": true
      },
      {
        "label": "sample false",
        "input": [
          "aabcc",
          "dbbca",
          "aadbbbaccc"
        ],
        "expected": false
      },
      {
        "label": "all empty",
        "input": [
          "",
          "",
          ""
        ],
        "expected": true
      },
      {
        "label": "length mismatch",
        "input": [
          "a",
          "b",
          "a"
        ],
        "expected": false
      },
      {
        "label": "greedy source choice fails",
        "input": [
          "aa",
          "ab",
          "aaba"
        ],
        "expected": true
      }
    ],
    "paramNames": [
      "s1",
      "s2",
      "s3"
    ],
    "paramTypes": [
      "string",
      "string",
      "string"
    ],
    "returnType": "boolean"
  },
  "best-time-to-buy-and-sell-stock-iii": {
    "kind": "function",
    "entry": "maxProfit",
    "checker": "exact",
    "cases": [
      {
        "label": "two transactions sample",
        "input": [
          [
            3,
            3,
            5,
            0,
            0,
            3,
            1,
            4
          ]
        ],
        "expected": 6
      },
      {
        "label": "rising prices need one transaction",
        "input": [
          [
            1,
            2,
            3,
            4,
            5
          ]
        ],
        "expected": 4
      },
      {
        "label": "falling prices",
        "input": [
          [
            7,
            6,
            4,
            3,
            1
          ]
        ],
        "expected": 0
      },
      {
        "label": "two small trades beat one wide trade",
        "input": [
          [
            1,
            4,
            2,
            7
          ]
        ],
        "expected": 8
      },
      {
        "label": "single day",
        "input": [
          [
            5
          ]
        ],
        "expected": 0
      }
    ],
    "paramNames": [
      "prices"
    ],
    "paramTypes": [
      "integer[]"
    ],
    "returnType": "integer"
  },
  "best-time-to-buy-and-sell-stock-iv": {
    "kind": "function",
    "entry": "maxProfit",
    "checker": "exact",
    "cases": [
      {
        "label": "single profitable trade",
        "input": [
          2,
          [
            2,
            4,
            1
          ]
        ],
        "expected": 2
      },
      {
        "label": "two transactions sample",
        "input": [
          2,
          [
            3,
            2,
            6,
            5,
            0,
            3
          ]
        ],
        "expected": 7
      },
      {
        "label": "zero transactions",
        "input": [
          0,
          [
            1,
            3,
            2,
            8
          ]
        ],
        "expected": 0
      },
      {
        "label": "large k behaves unlimited",
        "input": [
          100,
          [
            1,
            2,
            3,
            4,
            5
          ]
        ],
        "expected": 4
      },
      {
        "label": "k limits trades",
        "input": [
          1,
          [
            1,
            4,
            2,
            7
          ]
        ],
        "expected": 6
      }
    ],
    "paramNames": [
      "k",
      "prices"
    ],
    "paramTypes": [
      "integer",
      "integer[]"
    ],
    "returnType": "integer"
  },
  "sort-list": {
    "kind": "function",
    "entry": "sortList",
    "checker": "exact",
    "argTypes": [
      "linkedList"
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "sample reorder",
        "input": [
          [
            4,
            2,
            1,
            3
          ]
        ],
        "expected": [
          1,
          2,
          3,
          4
        ]
      },
      {
        "label": "negative and zero values",
        "input": [
          [
            -1,
            5,
            3,
            4,
            0
          ]
        ],
        "expected": [
          -1,
          0,
          3,
          4,
          5
        ]
      },
      {
        "label": "empty list",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "single node",
        "input": [
          [
            7
          ]
        ],
        "expected": [
          7
        ]
      },
      {
        "label": "duplicates preserved",
        "input": [
          [
            3,
            1,
            2,
            3,
            1
          ]
        ],
        "expected": [
          1,
          1,
          2,
          3,
          3
        ]
      }
    ],
    "paramNames": [
      "head"
    ],
    "paramTypes": [
      "ListNode"
    ],
    "returnType": "ListNode"
  },
  "median-of-two-sorted-arrays": {
    "kind": "function",
    "entry": "findMedianSortedArrays",
    "checker": "approxNumber",
    "cases": [
      {
        "label": "odd total sample",
        "input": [
          [
            1,
            3
          ],
          [
            2
          ]
        ],
        "expected": 2.0
      },
      {
        "label": "even total sample",
        "input": [
          [
            1,
            2
          ],
          [
            3,
            4
          ]
        ],
        "expected": 2.5
      },
      {
        "label": "left empty",
        "input": [
          [],
          [
            1
          ]
        ],
        "expected": 1.0
      },
      {
        "label": "negative values even total",
        "input": [
          [
            -5,
            -3
          ],
          [
            -4,
            -1
          ]
        ],
        "expected": -3.5
      },
      {
        "label": "partition must handle imbalance",
        "input": [
          [
            1,
            2
          ],
          [
            3,
            4,
            5,
            6,
            7
          ]
        ],
        "expected": 4.0
      }
    ],
    "paramNames": [
      "nums1",
      "nums2"
    ],
    "paramTypes": [
      "integer[]",
      "integer[]"
    ],
    "returnType": "double"
  },
  "max-points-on-a-line": {
    "kind": "function",
    "entry": "maxPoints",
    "checker": "exact",
    "cases": [
      {
        "label": "diagonal sample",
        "input": [
          [
            [
              1,
              1
            ],
            [
              2,
              2
            ],
            [
              3,
              3
            ]
          ]
        ],
        "expected": 3
      },
      {
        "label": "mixed slope sample",
        "input": [
          [
            [
              1,
              1
            ],
            [
              3,
              2
            ],
            [
              5,
              3
            ],
            [
              4,
              1
            ],
            [
              2,
              3
            ],
            [
              1,
              4
            ]
          ]
        ],
        "expected": 4
      },
      {
        "label": "single point",
        "input": [
          [
            [
              0,
              0
            ]
          ]
        ],
        "expected": 1
      },
      {
        "label": "vertical line",
        "input": [
          [
            [
              2,
              1
            ],
            [
              2,
              3
            ],
            [
              2,
              5
            ],
            [
              4,
              5
            ]
          ]
        ],
        "expected": 3
      },
      {
        "label": "reduced slope duplicates avoided",
        "input": [
          [
            [
              0,
              0
            ],
            [
              2,
              2
            ],
            [
              4,
              4
            ],
            [
              1,
              2
            ],
            [
              2,
              4
            ]
          ]
        ],
        "expected": 3
      }
    ],
    "paramNames": [
      "points"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "integer"
  },
  "reverse-bits": {
    "kind": "function",
    "entry": "reverseBits",
    "checker": "exact",
    "cases": [
      {
        "label": "sample bit pattern",
        "input": [
          43261596
        ],
        "expected": 964176192
      },
      {
        "label": "zero",
        "input": [
          0
        ],
        "expected": 0
      },
      {
        "label": "single low-safe bit",
        "input": [
          2
        ],
        "expected": 1073741824
      },
      {
        "label": "two low-safe bits",
        "input": [
          6
        ],
        "expected": 1610612736
      },
      {
        "label": "alternating pattern signed-safe",
        "input": [
          715827882
        ],
        "expected": 1431655764
      }
    ],
    "paramNames": [
      "n"
    ],
    "paramTypes": [
      "integer"
    ],
    "returnType": "integer"
  },
  "ipo": {
    "kind": "function",
    "entry": "findMaximizedCapital",
    "checker": "exact",
    "cases": [
      {
        "label": "sample unlocks better project",
        "input": [
          2,
          0,
          [
            1,
            2,
            3
          ],
          [
            0,
            1,
            1
          ]
        ],
        "expected": 4
      },
      {
        "label": "chain unlocks all projects",
        "input": [
          3,
          0,
          [
            1,
            2,
            3
          ],
          [
            0,
            1,
            2
          ]
        ],
        "expected": 6
      },
      {
        "label": "no affordable project",
        "input": [
          2,
          0,
          [
            5,
            6
          ],
          [
            10,
            12
          ]
        ],
        "expected": 0
      },
      {
        "label": "choose best affordable not first",
        "input": [
          1,
          2,
          [
            1,
            100,
            3
          ],
          [
            0,
            2,
            2
          ]
        ],
        "expected": 102
      },
      {
        "label": "stop after k projects",
        "input": [
          2,
          1,
          [
            2,
            4,
            8
          ],
          [
            0,
            1,
            3
          ]
        ],
        "expected": 13
      }
    ],
    "paramNames": [
      "k",
      "w",
      "profits",
      "capital"
    ],
    "paramTypes": [
      "integer",
      "integer",
      "integer[]",
      "integer[]"
    ],
    "returnType": "integer"
  },
  "find-k-pairs-with-smallest-sums": {
    "kind": "function",
    "entry": "kSmallestPairs",
    "checker": "integerRows",
    "cases": [
      {
        "label": "sample first row wins",
        "input": [
          [
            1,
            7,
            11
          ],
          [
            2,
            4,
            6
          ],
          3
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            1,
            4
          ],
          [
            1,
            6
          ]
        ]
      },
      {
        "label": "duplicate pairs preserved",
        "input": [
          [
            1,
            1,
            2
          ],
          [
            1,
            2,
            3
          ],
          2
        ],
        "expected": [
          [
            1,
            1
          ],
          [
            1,
            1
          ]
        ]
      },
      {
        "label": "k exceeds pair count",
        "input": [
          [
            1
          ],
          [
            2,
            3
          ],
          5
        ],
        "expected": [
          [
            1,
            2
          ],
          [
            1,
            3
          ]
        ]
      },
      {
        "label": "negative values",
        "input": [
          [
            -2,
            -1
          ],
          [
            3,
            4
          ],
          3
        ],
        "expected": [
          [
            -2,
            3
          ],
          [
            -2,
            4
          ],
          [
            -1,
            3
          ]
        ]
      },
      {
        "label": "single requested pair",
        "input": [
          [
            1,
            2
          ],
          [
            3,
            4
          ],
          1
        ],
        "expected": [
          [
            1,
            3
          ]
        ]
      }
    ],
    "paramNames": [
      "nums1",
      "nums2",
      "k"
    ],
    "paramTypes": [
      "integer[]",
      "integer[]",
      "integer"
    ],
    "returnType": "list<list<integer>>"
  },
  "merge-k-sorted-lists": {
    "kind": "function",
    "entry": "mergeKLists",
    "checker": "exact",
    "argTypes": [
      "linkedListArray"
    ],
    "outputType": "linkedList",
    "cases": [
      {
        "label": "three interleaved lists",
        "input": [
          [
            [
              1,
              4,
              5
            ],
            [
              1,
              3,
              4
            ],
            [
              2,
              6
            ]
          ]
        ],
        "expected": [
          1,
          1,
          2,
          3,
          4,
          4,
          5,
          6
        ]
      },
      {
        "label": "no lists",
        "input": [
          []
        ],
        "expected": []
      },
      {
        "label": "one empty list",
        "input": [
          [
            []
          ]
        ],
        "expected": []
      },
      {
        "label": "negative values",
        "input": [
          [
            [
              -3,
              -1,
              2
            ],
            [
              -2,
              4
            ]
          ]
        ],
        "expected": [
          -3,
          -2,
          -1,
          2,
          4
        ]
      },
      {
        "label": "duplicate values across lists",
        "input": [
          [
            [
              1,
              1
            ],
            [
              1
            ],
            [
              1,
              2
            ]
          ]
        ],
        "expected": [
          1,
          1,
          1,
          1,
          2
        ]
      }
    ],
    "paramNames": [
      "lists"
    ],
    "paramTypes": [
      "ListNode[]"
    ],
    "returnType": "ListNode"
  },
  "find-median-from-data-stream": {
    "kind": "class",
    "className": "MedianFinder",
    "checker": "exact",
    "cases": [
      {
        "label": "sample odd after even",
        "input": [
          [
            "MedianFinder",
            "addNum",
            "addNum",
            "findMedian",
            "addNum",
            "findMedian"
          ],
          [
            [],
            [
              1
            ],
            [
              2
            ],
            [],
            [
              3
            ],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          1.5,
          null,
          2.0
        ]
      },
      {
        "label": "negative and positive values",
        "input": [
          [
            "MedianFinder",
            "addNum",
            "addNum",
            "findMedian",
            "addNum",
            "findMedian"
          ],
          [
            [],
            [
              -1
            ],
            [
              -2
            ],
            [],
            [
              3
            ],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          -1.5,
          null,
          -1.0
        ]
      },
      {
        "label": "duplicates keep median stable",
        "input": [
          [
            "MedianFinder",
            "addNum",
            "addNum",
            "addNum",
            "findMedian"
          ],
          [
            [],
            [
              5
            ],
            [
              5
            ],
            [
              5
            ],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          5.0
        ]
      },
      {
        "label": "even average after four values",
        "input": [
          [
            "MedianFinder",
            "addNum",
            "addNum",
            "addNum",
            "addNum",
            "findMedian"
          ],
          [
            [],
            [
              10
            ],
            [
              20
            ],
            [
              30
            ],
            [
              40
            ],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          null,
          null,
          25.0
        ]
      },
      {
        "label": "median shifts down",
        "input": [
          [
            "MedianFinder",
            "addNum",
            "addNum",
            "findMedian",
            "addNum",
            "findMedian"
          ],
          [
            [],
            [
              100
            ],
            [
              1
            ],
            [],
            [
              2
            ],
            []
          ]
        ],
        "expected": [
          null,
          null,
          null,
          50.5,
          null,
          2.0
        ]
      }
    ]
  },
  "construct-quad-tree": {
    "kind": "function",
    "entry": "construct",
    "paramNames": [
      "grid"
    ],
    "paramTypes": [
      "integer[][]"
    ],
    "returnType": "Node",
    "checker": "exact",
    "outputType": "quadTree",
    "cases": [
      {
        "label": "single one",
        "input": [
          [
            [
              1
            ]
          ]
        ],
        "expected": [
          [
            1,
            1
          ]
        ]
      },
      {
        "label": "single zero",
        "input": [
          [
            [
              0
            ]
          ]
        ],
        "expected": [
          [
            1,
            0
          ]
        ]
      },
      {
        "label": "checkerboard",
        "input": [
          [
            [
              0,
              1
            ],
            [
              1,
              0
            ]
          ]
        ],
        "expected": [
          [
            0,
            1
          ],
          [
            1,
            0
          ],
          [
            1,
            1
          ],
          [
            1,
            1
          ],
          [
            1,
            0
          ]
        ]
      },
      {
        "label": "uniform two by two",
        "input": [
          [
            [
              1,
              1
            ],
            [
              1,
              1
            ]
          ]
        ],
        "expected": [
          [
            1,
            1
          ]
        ]
      },
      {
        "label": "mixed four by four",
        "input": [
          [
            [
              1,
              1,
              0,
              0
            ],
            [
              1,
              1,
              0,
              0
            ],
            [
              1,
              0,
              0,
              0
            ],
            [
              0,
              0,
              0,
              0
            ]
          ]
        ],
        "expected": [
          [
            0,
            1
          ],
          [
            1,
            1
          ],
          [
            1,
            0
          ],
          [
            0,
            1
          ],
          [
            1,
            1
          ],
          [
            1,
            0
          ],
          [
            1,
            0
          ],
          [
            1,
            0
          ],
          [
            1,
            0
          ]
        ]
      }
    ]
  }
};
