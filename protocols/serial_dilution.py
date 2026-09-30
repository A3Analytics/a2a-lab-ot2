from opentrons import protocol_api

metadata = {
    "protocolName": "Serial Dilution (missing tip)",
    "description": "OT-2 serial dilution that fails mid-run: diluent and stock mix succeed, then the next aspirate runs without a tip.",
    "author": "A3 Analytics",
}

requirements = {"robotType": "OT-2", "apiLevel": "2.16"}


def run(protocol: protocol_api.ProtocolContext) -> None:
    tips = protocol.load_labware("opentrons_96_tiprack_300ul", 1)
    reservoir = protocol.load_labware("nest_12_reservoir_15ml", 2)
    plate = protocol.load_labware("nest_96_wellplate_200ul_flat", 3)
    pipette = protocol.load_instrument("p300_single", "right", tip_racks=[tips])

    diluent = reservoir["A1"]
    stock = reservoir["A2"]
    row = plate.rows()[0][:3]

    pipette.pick_up_tip()
    pipette.transfer(100, diluent, row, new_tip="never")
    pipette.transfer(50, stock, row[0], new_tip="never", mix_after=(3, 40))
    pipette.drop_tip()
    pipette.aspirate(50, row[0])
    pipette.dispense(50, row[1])
    pipette.mix(3, 40, row[1])
    pipette.transfer(50, row[1], row[2], new_tip="never", mix_after=(3, 40))
