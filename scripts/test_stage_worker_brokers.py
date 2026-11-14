"""Synthetic packaging guards; no signing or trusted runtime capability."""
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import stage_worker_brokers as staging
from worker_broker_manifest import MAGIC, ROLES, parse


class BrokerBundleTests(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.payload=Path(self.directory.name)/"worker-brokers"
        self.payload.mkdir()
        with patch.object(staging,"PAYLOAD",self.payload):
            staging.stage_sources()
        self.services=self.payload/"services"
        records=[MAGIC]
        for role in ROLES:
            service=self.services/f"{role.service}.xpc"
            macos=service/"Contents/MacOS"
            macos.mkdir(parents=True)
            signature=service/"Contents/_CodeSignature"
            signature.mkdir()
            (signature/"CodeResources").write_bytes(b"synthetic signature, no authority")
            (service/"Contents/Info.plist").write_bytes(plistlib.dumps(staging.info(role)))
            broker=macos/role.executable
            worker=macos/role.worker
            broker.write_bytes(f"synthetic {role.name} broker".encode())
            worker.write_bytes(f"synthetic {role.name} worker".encode())
            records.append(f"{role.name} {role.service} {role.executable} {staging.digest(broker)} {'b'*40} {role.worker} {staging.digest(worker)} {'c'*40}")
        self.manifest="\n".join(records)+"\n"
        (self.payload/"manifest.txt").write_text(self.manifest)

    @staticmethod
    def identity(path):
        return staging.digest(path), ("c" if path.name in {role.worker for role in ROLES} else "b")*40

    def verify(self, entitlements=b""):
        with (patch.object(staging,"identity",side_effect=self.identity),
              patch.object(staging,"assert_executable"),
              patch.object(staging.subprocess,"run",return_value=subprocess.CompletedProcess([],0,stdout=entitlements))):
            return staging.verify_services(self.payload,self.services)

    def test_complete_fixed_role_cohort_and_source_license_pass(self):
        self.assertEqual(len(self.verify()),3)

    def test_role_order_unknown_names_and_noncanonical_pins_reject(self):
        lines=self.manifest.splitlines()
        for text in [self.manifest.replace("compiler ","unknown ",1),
                     "\n".join([lines[0],lines[2],lines[1],lines[3]])+"\n",
                     self.manifest.replace("b"*40,"B"*40,1), self.manifest+"extra\n"]:
            with self.assertRaises(ValueError): parse(text)

    def test_replaced_worker_indirection_and_extra_bundle_files_reject(self):
        role=ROLES[0]
        worker=self.services/f"{role.service}.xpc/Contents/MacOS/{role.worker}"
        original=worker.read_bytes()
        worker.write_bytes(b"replacement")
        with self.assertRaises(ValueError): self.verify()
        worker.write_bytes(original)
        extra=worker.parent/"unrequested-input"
        extra.write_bytes(b"not permitted")
        with self.assertRaises(ValueError): self.verify()
        extra.unlink()
        worker.unlink()
        worker.symlink_to(worker.parent/role.executable)
        with self.assertRaises(ValueError): self.verify()

    def test_new_entitlements_and_mutable_source_lock_cannot_expand_policy(self):
        with self.assertRaises(ValueError):
            self.verify(plistlib.dumps({"com.apple.security.network.client":True}))
        source=self.payload/"source/broker/profile.c"
        source.write_text("replacement profile")
        lock=self.payload/"source.sha256"
        entries=[]
        for line in lock.read_text().splitlines():
            _,name=line.split("  ")
            entries.append(f"{staging.digest(self.payload/name)}  {name}\n")
        lock.write_text("".join(entries))
        with self.assertRaises(ValueError): self.verify()

    def test_service_metadata_cannot_enable_a_different_launch_contract(self):
        role=ROLES[1]
        path=self.services/f"{role.service}.xpc/Contents/Info.plist"
        changed=staging.info(role)
        changed["XPCService"]["ServiceType"]="System"
        path.write_bytes(plistlib.dumps(changed))
        with self.assertRaises(ValueError): self.verify()

    def test_bundle_compares_real_macho_reader_lines_and_rejects_foreign_host_pins(self):
        app=Path(self.directory.name)/"App.app"
        payload=app/"Contents/Resources/worker-brokers"
        payload.parent.mkdir(parents=True)
        shutil.copytree(self.payload,payload)
        macos=app/"Contents/MacOS"
        macos.mkdir()
        host=macos/"zeta-desktop"
        def image(text):
            data=text.encode("ascii")
            segment=struct.pack("<II16sQQQQiiII",0x19,152,b"__TEXT",0,0,0,0,7,5,1,0)
            section=struct.pack("<16s16sQQIIIIIIII",b"__worker_brokers",b"__TEXT",0,len(data),184,0,0,0,0,0,0,0)
            header=struct.pack("<IiiIIIII",0xFEEDFACF,0x0100000C,0,2,1,152,0,0)
            return header+segment+section+data
        host.write_bytes(image(self.manifest))
        with (patch.object(staging,"identity",side_effect=self.identity),
              patch.object(staging,"assert_executable"),
              patch.object(staging.subprocess,"run",return_value=subprocess.CompletedProcess([],0,stdout=b""))):
            self.assertEqual(staging.embedded_manifest(host,b"__worker_brokers"),self.manifest.splitlines())
            self.assertEqual(len(staging.verify_bundle(app,unsigned=True)),3)
            shutil.move(str(payload/"services"),app/"Contents/XPCServices")
            self.assertEqual(len(staging.verify_bundle(app)),3)
            host.write_bytes(image(self.manifest.replace("b"*40,"e"*40,1)))
            with self.assertRaises(ValueError): staging.verify_bundle(app)


if __name__ == "__main__":
    unittest.main()
