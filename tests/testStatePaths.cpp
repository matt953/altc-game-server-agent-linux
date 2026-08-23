#include <catch2/catch_test_macros.hpp>
#include <catch2/matchers/catch_matchers_string.hpp>
#include <helpers/utils.hpp>

using Catch::Matchers::Equals;

TEST_CASE("sanitize_path_component", "[utils]") {
  SECTION("titles that are already safe are untouched") {
    // Existing state folders must keep resolving to the same path.
    REQUIRE_THAT(utils::sanitize_path_component("Baldur's Gate 3"), Equals("Baldur's Gate 3"));
    REQUIRE_THAT(utils::sanitize_path_component("No Man's Sky"), Equals("No Man's Sky"));
    REQUIRE_THAT(utils::sanitize_path_component("Wolfenstein - The New Order"), Equals("Wolfenstein - The New Order"));
    REQUIRE_THAT(utils::sanitize_path_component("Ünïcödé 日本語"), Equals("Ünïcödé 日本語"));
  }

  SECTION("a ':' would split a docker bind and is replaced") {
    REQUIRE_THAT(utils::sanitize_path_component("Wolfenstein: The New Order"), Equals("Wolfenstein_ The New Order"));
  }

  SECTION("separators can't escape the parent folder") {
    REQUIRE_THAT(utils::sanitize_path_component("../../etc/passwd"), Equals(".._.._etc_passwd"));
    REQUIRE_THAT(utils::sanitize_path_component("a\\b"), Equals("a_b"));
  }

  SECTION("control characters are replaced") {
    REQUIRE_THAT(utils::sanitize_path_component(std::string("tab\there\n")), Equals("tab_here_"));
    REQUIRE_THAT(utils::sanitize_path_component(std::string("del\x7f")), Equals("del_"));
  }

  SECTION("names that are not usable as a folder get a prefix") {
    REQUIRE_THAT(utils::sanitize_path_component(""), Equals("_"));
    REQUIRE_THAT(utils::sanitize_path_component("."), Equals("_."));
    REQUIRE_THAT(utils::sanitize_path_component(".."), Equals("_.."));
  }
}

TEST_CASE("is_safe_state_folder", "[utils]") {
  SECTION("accepts relative paths, including nested ones") {
    REQUIRE(utils::is_safe_state_folder("lobby-1"));
    REQUIRE(utils::is_safe_state_folder("clients/1234/lobby-1"));
    REQUIRE(utils::is_safe_state_folder("a..b"));
  }

  SECTION("rejects traversal in any segment") {
    REQUIRE_FALSE(utils::is_safe_state_folder(".."));
    REQUIRE_FALSE(utils::is_safe_state_folder("../etc"));
    REQUIRE_FALSE(utils::is_safe_state_folder("lobby/../../etc"));
    REQUIRE_FALSE(utils::is_safe_state_folder("lobby/.."));
  }

  SECTION("rejects absolute paths, ':' and empty") {
    REQUIRE_FALSE(utils::is_safe_state_folder("/etc/wolf"));
    REQUIRE_FALSE(utils::is_safe_state_folder("lobby:1"));
    REQUIRE_FALSE(utils::is_safe_state_folder(""));
  }
}
