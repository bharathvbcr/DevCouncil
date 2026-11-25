//go:build windows

package dc

import (
	"fmt"
	"os"
	"syscall"
	"unsafe"
)

// Windows reports 0666 for every writable file, so the Unix mode check cannot
// tell a private key from a public one there. The access control is the DACL,
// so that is what is read here: a key file is accepted only when its DACL
// grants access to the file's owner alone.
//
// Decision on SYSTEM (S-1-5-18) and Administrators (S-1-5-32-544): they MAY
// hold access. An administrator can take ownership of any file and read it
// regardless of its DACL, and SYSTEM is the OS itself, so refusing them would
// reject the ordinary DACL of a file created under a user profile, where both
// are inherited, without protecting the key from anyone who matters. OWNER
// RIGHTS (S-1-3-4), the well-known SID that stands for "whoever owns the
// file", is the owner by definition and is allowed for the same reason.
// Everyone, Authenticated Users and Users are refused by name, and so is every
// other SID.

const (
	seFileObject            = 1 // SE_FILE_OBJECT
	ownerSecurityInformatio = 0x00000001
	daclSecurityInformation = 0x00000004
	sddlRevision1           = 1

	aceTypeAccessAllowed       = 0x00
	aceTypeAccessDenied        = 0x01
	aceTypeAccessDeniedObject  = 0x06
	aceTypeAccessDeniedCB      = 0x0A
	aceTypeAccessDeniedObjCB   = 0x0C
	aceFlagInheritOnly         = 0x08
	sidMinimumLength           = 8 // revision, sub-authority count, 6-byte authority
	aceAllowedFixedPrefixBytes = 8 // ACE_HEADER (4) + Mask (4); the SID follows

	sidEveryone           = "S-1-1-0"
	sidAuthenticatedUsers = "S-1-5-11"
	sidUsers              = "S-1-5-32-545"
	sidSystem             = "S-1-5-18"
	sidAdministrators     = "S-1-5-32-544"
	sidOwnerRights        = "S-1-3-4"

	// ownerOnlySDDL is a protected DACL (no inherited entries) with one ACE:
	// full access for OWNER RIGHTS, which the system resolves to whoever owns
	// the file, so it holds for an elevated token whose default owner is the
	// Administrators group as well as for an ordinary user.
	ownerOnlySDDL = "D:P(A;;FA;;;OW)"
)

var (
	advapi32                  = syscall.NewLazyDLL("advapi32.dll")
	kernel32                  = syscall.NewLazyDLL("kernel32.dll")
	procGetNamedSecurityInfoW = advapi32.NewProc("GetNamedSecurityInfoW")
	procGetAce                = advapi32.NewProc("GetAce")
	procConvertStringSDToSDW  = advapi32.NewProc("ConvertStringSecurityDescriptorToSecurityDescriptorW")
	procLocalFree             = kernel32.NewProc("LocalFree")
)

// aclHeader is the ACL structure that begins every DACL.
type aclHeader struct {
	AclRevision byte
	Sbz1        byte
	AclSize     uint16
	AceCount    uint16
	Sbz2        uint16
}

// aceHeader is ACE_HEADER, the start of every access control entry.
type aceHeader struct {
	AceType  byte
	AceFlags byte
	AceSize  uint16
}

// accessAllowedACE is ACCESS_ALLOWED_ACE. SidStart is only the first DWORD of
// the SID, which runs on past the struct for as many bytes as the SID needs.
type accessAllowedACE struct {
	Header   aceHeader
	Mask     uint32
	SidStart uint32
}

func localFree(p unsafe.Pointer) {
	if p != nil {
		_, _, _ = procLocalFree.Call(uintptr(p))
	}
}

// sidString renders the SID at p, which must lie wholly within limit bytes.
func sidString(p unsafe.Pointer, limit int) (string, error) {
	if limit < sidMinimumLength {
		return "", fmt.Errorf("a SID cannot fit in %d bytes", limit)
	}
	// SAFETY: p points at least sidMinimumLength readable bytes (checked
	// above); byte 1 of a SID is its sub-authority count.
	subAuthorities := int(*(*byte)(unsafe.Add(p, 1)))
	if sidMinimumLength+4*subAuthorities > limit {
		return "", fmt.Errorf("a SID with %d sub-authorities does not fit in %d bytes", subAuthorities, limit)
	}
	// SAFETY: the SID was just shown to lie within the buffer the caller owns,
	// and String only reads it.
	return (*syscall.SID)(p).String()
}

// checkKeyFileAccess accepts the key file only when its DACL grants access to
// its owner alone (plus SYSTEM and Administrators, see above). Every failure to
// read or understand the DACL refuses the key.
func checkKeyFileAccess(path string, _ os.FileInfo) error {
	name, err := syscall.UTF16PtrFromString(path)
	if err != nil {
		return fmt.Errorf("%s: %w", path, err)
	}
	var owner, dacl, descriptor unsafe.Pointer
	// SAFETY: every out-parameter is the address of a local pointer. On success
	// owner and dacl point inside the single buffer descriptor, which is freed
	// below and not used after.
	r, _, _ := procGetNamedSecurityInfoW.Call(
		uintptr(unsafe.Pointer(name)),
		seFileObject,
		ownerSecurityInformatio|daclSecurityInformation,
		uintptr(unsafe.Pointer(&owner)),
		0,
		uintptr(unsafe.Pointer(&dacl)),
		0,
		uintptr(unsafe.Pointer(&descriptor)),
	)
	if r != 0 {
		return fmt.Errorf("%s: cannot read the file's security descriptor, so its access cannot be checked: %w", path, syscall.Errno(r))
	}
	defer localFree(descriptor)
	if owner == nil {
		return fmt.Errorf("%s: the file has no owner SID, so its access cannot be checked", path)
	}
	if dacl == nil {
		return fmt.Errorf("%s has no DACL, which grants everyone access; a signing key must be readable by its owner alone", path)
	}
	ownerSID, err := sidString(owner, 8+4*255)
	if err != nil {
		return fmt.Errorf("%s: owner SID: %w", path, err)
	}

	// SAFETY: dacl points at an ACL the system returned; its header is the
	// first sizeof(aclHeader) bytes.
	header := (*aclHeader)(dacl)
	for i := 0; i < int(header.AceCount); i++ {
		var ace unsafe.Pointer
		// SAFETY: ace receives a pointer into the DACL buffer, valid while
		// descriptor is unfreed.
		ok, _, callErr := procGetAce.Call(uintptr(dacl), uintptr(i), uintptr(unsafe.Pointer(&ace)))
		if ok == 0 || ace == nil {
			return fmt.Errorf("%s: cannot read ACE %d of its DACL: %w", path, i, callErr)
		}
		// SAFETY: every ACE begins with an ACE_HEADER.
		h := (*aceHeader)(ace)
		if h.AceFlags&aceFlagInheritOnly != 0 {
			continue // applies to children of this object, not to the file
		}
		switch h.AceType {
		case aceTypeAccessDenied, aceTypeAccessDeniedObject, aceTypeAccessDeniedCB, aceTypeAccessDeniedObjCB:
			continue // a deny entry only narrows access
		case aceTypeAccessAllowed:
		default:
			return fmt.Errorf("%s: ACE %d has type %#x, which is not understood, so the file's access cannot be proven owner-only", path, i, h.AceType)
		}
		if int(h.AceSize) < aceAllowedFixedPrefixBytes+sidMinimumLength {
			return fmt.Errorf("%s: ACE %d is %d bytes, too short to hold a SID", path, i, h.AceSize)
		}
		// SAFETY: the ACE is AceSize bytes, at least the fixed prefix (checked
		// above), so the mask is readable; the SID starts at SidStart.
		allowed := (*accessAllowedACE)(ace)
		sid, err := sidString(unsafe.Add(ace, aceAllowedFixedPrefixBytes), int(h.AceSize)-aceAllowedFixedPrefixBytes)
		if err != nil {
			return fmt.Errorf("%s: ACE %d: %w", path, i, err)
		}
		if allowed.Mask == 0 {
			continue // grants nothing
		}
		switch sid {
		case sidEveryone, sidAuthenticatedUsers, sidUsers:
			return fmt.Errorf("%s grants access to %s; a signing key must be readable by its owner alone", path, sid)
		case ownerSID, sidSystem, sidAdministrators, sidOwnerRights:
		default:
			return fmt.Errorf("%s grants access to %s, who is not its owner; a signing key must be readable by its owner alone", path, sid)
		}
	}
	return nil
}

// CreateSigningKeyFile creates a new private key file for writing. It never
// overwrites an existing file (CREATE_NEW), and the file is created with a
// protected owner-only DACL, so it is owner-only from the moment it exists and
// before any secret byte is written.
func CreateSigningKeyFile(path string) (*os.File, error) {
	return createFileWithSDDL(path, ownerOnlySDDL)
}

// createFileWithSDDL creates a new file whose security descriptor is the one
// sddl describes. Tests use it to make files with a chosen DACL.
func createFileWithSDDL(path, sddl string) (*os.File, error) {
	name, err := syscall.UTF16PtrFromString(path)
	if err != nil {
		return nil, err
	}
	text, err := syscall.UTF16PtrFromString(sddl)
	if err != nil {
		return nil, err
	}
	var descriptor unsafe.Pointer
	// SAFETY: descriptor receives a LocalAlloc'd security descriptor, freed
	// below after the file exists; the size out-parameter is not wanted.
	r, _, callErr := procConvertStringSDToSDW.Call(
		uintptr(unsafe.Pointer(text)), sddlRevision1, uintptr(unsafe.Pointer(&descriptor)), 0)
	if r == 0 {
		return nil, fmt.Errorf("security descriptor %q: %w", sddl, callErr)
	}
	defer localFree(descriptor)
	attrs := syscall.SecurityAttributes{
		Length: uint32(unsafe.Sizeof(syscall.SecurityAttributes{})),
		// SAFETY: descriptor is a valid security descriptor that outlives the
		// CreateFile call below.
		SecurityDescriptor: uintptr(descriptor),
	}
	handle, err := syscall.CreateFile(name, syscall.GENERIC_WRITE, 0, &attrs,
		syscall.CREATE_NEW, syscall.FILE_ATTRIBUTE_NORMAL, 0)
	if err != nil {
		return nil, &os.PathError{Op: "open", Path: path, Err: err}
	}
	return os.NewFile(uintptr(handle), path), nil
}
