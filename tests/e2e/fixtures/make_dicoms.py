"""Generate small synthetic DICOM files for the e2e tests.

Every value is fake. Output: one Part 10 angiography image per entry of PATIENTS x
STUDIES below (8x8 MONOCHROME2 pixels), plus manifest.json describing each file so the
tests never hard-code what is inside.

Names all start with ZZTEST so a test can assert that no trace of them survives
deidentification.
"""
import json
import sys
from pathlib import Path

from pydicom.dataset import Dataset, FileMetaDataset
from pydicom.uid import (
    ExplicitVRLittleEndian,
    SecondaryCaptureImageStorage,
    XRayAngiographicImageStorage,
    generate_uid,
)

UID_ROOT = "1.2.826.0.1.3680043.10.543.999."  # fixed prefix, deterministic suffixes

PATIENTS = [
    # key, PatientID (None = element absent, "" = present but empty), name, birth date, sex
    ("p1000001", "1000001", "ZZTEST^ALICE", "19540212", "F"),
    ("p1000002", "1000002", "ZZTEST^BRUNO", "19601105", "M"),
    ("noid", None, "ZZTEST^CHARLES", "19720330", "M"),
    ("emptyid", "", "ZZTEST^DIANE", "19810719", "F"),
    # Active HTML, as any file may carry: it must reach the page as text.
    ("html", "1000003", "ZZTEST<img src=x onerror=window.__xss=1>^EVE", "19900101", "F"),
    # A report wrapped in DICOM, of a patient who also has images.
    ("report", "1000001", "ZZTEST^ALICE", "19540212", "F"),
    # A screen capture, refused like the report.
    ("capture", "1000001", "ZZTEST^ALICE", "19540212", "F"),
    # A reconstruction, left out of the batch.
    ("derived", "1000001", "ZZTEST^ALICE", "19540212", "F"),
    # Fluoroscopy: processed pixels, but an acquisition, which goes through.
    ("fluoro", "1000001", "ZZTEST^ALICE", "19540212", "F"),
]

STUDIES = [
    # key, StudyDate, StudyTime
    ("20240115", "20240115", "093000"),
    ("20240215", "20240215", "141500"),
]


def uid(*parts):
    return generate_uid(prefix=UID_ROOT, entropy_srcs=[str(p) for p in parts])


def make(patient, study):
    pkey, pid, name, birth, sex = patient
    skey, study_date, study_time = study

    meta = FileMetaDataset()
    meta.MediaStorageSOPClassUID = (
        SecondaryCaptureImageStorage if pkey == "capture" else XRayAngiographicImageStorage
    )
    meta.MediaStorageSOPInstanceUID = uid(pkey, skey, "sop")
    meta.TransferSyntaxUID = ExplicitVRLittleEndian
    meta.ImplementationClassUID = UID_ROOT + "1"

    ds = Dataset()
    ds.file_meta = meta
    ds.SOPClassUID = meta.MediaStorageSOPClassUID
    ds.SOPInstanceUID = meta.MediaStorageSOPInstanceUID
    ds.StudyInstanceUID = uid(pkey, skey, "study")
    ds.SeriesInstanceUID = uid(pkey, skey, "series")
    ds.Modality = "XA"
    if pkey == "html":
        ds.SeriesDescription = "<img src=x onerror=window.__xss=2>"
    ds.ImageType = {
        "derived": ["DERIVED", "SECONDARY", "MPR"],
        "fluoro": ["DERIVED", "PRIMARY", "SINGLE PLANE"],
    }.get(pkey, ["ORIGINAL", "PRIMARY", "SINGLE PLANE"])
    if pkey == "report":
        ds.MIMETypeOfEncapsulatedDocument = "application/pdf"
        ds.EncapsulatedDocument = b"%PDF-1.4 ZZTEST "
    ds.PatientName = name
    if pid is not None:
        ds.PatientID = pid
    ds.PatientBirthDate = birth
    ds.PatientSex = sex
    ds.StudyDate = study_date
    ds.StudyTime = study_time
    ds.SeriesDate = study_date
    ds.ContentDate = study_date
    ds.AccessionNumber = "ZZACC" + skey
    ds.InstitutionName = "ZZTEST HOSPITAL"
    ds.ReferringPhysicianName = "ZZTEST^DOCTOR"
    ds.StudyID = "1"
    ds.SeriesNumber = 1
    ds.InstanceNumber = 1
    ds.SamplesPerPixel = 1
    ds.PhotometricInterpretation = "MONOCHROME2"
    ds.Rows = 8
    ds.Columns = 8
    ds.BitsAllocated = 8
    ds.BitsStored = 8
    ds.HighBit = 7
    ds.PixelRepresentation = 0
    ds.PixelData = bytes((r * 8 + c) * 4 for r in range(8) for c in range(8))

    return ds, {
        "patientKey": pkey,
        "patientId": pid,
        "patientName": name,
        "birthDate": birth,
        "sex": sex,
        "studyDate": study_date,
        "studyTime": study_time,
        "studyInstanceUID": ds.StudyInstanceUID,
        "sopInstanceUID": ds.SOPInstanceUID,
    }


def main(out_dir):
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for patient in PATIENTS:
        for study in STUDIES:
            ds, info = make(patient, study)
            file_name = f"{patient[0]}_{study[0]}.dcm"
            ds.save_as(out / file_name, enforce_file_format=True)
            info["size"] = (out / file_name).stat().st_size
            manifest[file_name] = info
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"wrote {len(manifest)} DICOM files + manifest.json to {out}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "out")
