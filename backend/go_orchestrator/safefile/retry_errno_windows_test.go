package safefile

// errorSharingViolationForTest is ERROR_SHARING_VIOLATION, the refusal an open
// meets while another handle renames over the file.
var errorSharingViolationForTest error = errorSharingViolation
