import unittest

from rust_imports import crate_roots


class CrateRootsTests(unittest.TestCase):
    def test_simple_and_grouped_imports(self):
        source = "use crate::model::Thing; use crate::{catalog, study::View};"
        self.assertEqual(crate_roots(source), {"model", "catalog", "study"})

    def test_nested_groups_and_aliases(self):
        source = "use crate :: { self, model::{Thing, Other}, plan as schedule };"
        self.assertEqual(crate_roots(source), {"model", "plan"})

    def test_grouped_disallowed_edge_cannot_be_hidden(self):
        allowed = {"model"}
        self.assertFalse(crate_roots("use crate::{model, study};") <= allowed)


if __name__ == "__main__":
    unittest.main()
