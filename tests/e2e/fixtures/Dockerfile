# Throwaway generator for synthetic DICOM fixtures: ./run.sh fixtures
FROM python:3.12-slim
RUN pip install --no-cache-dir "pydicom==3.0.1"
COPY make_dicoms.py /make_dicoms.py
ENTRYPOINT ["python", "-I", "/make_dicoms.py", "/out"]
