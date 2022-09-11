-- Put this in your Wireshark/plugins/ folder and it should show up as an
-- option in Right click on message -> Decode As -> COMMS

local comms_protocol = Proto("Comms", "Comms Package Data")
local kind_field = ProtoField.new("kind", "comms.kind", ftypes.UINT8)
local id_field = ProtoField.new("id", "comms.id", ftypes.UINT64)
local body_len_field = ProtoField.new("body_len", "comms.body_len", ftypes.UINT16)
local reply_to = ProtoField.new("reply_to", "comms.reply_to", ftypes.BYTES)
local padding = ProtoField.new("padding", "comms.padding", ftypes.BYTES)
local body = ProtoField.new("body", "comms.body", ftypes.BYTES)

local expert_unknown_kind = ProtoExpert.new("comms.unknown_kind", "Package Kind is not known",
    expert.group.MALFORMED,
    expert.severity.ERROR);

local expert_udp_kind_id_used_in_tcp = ProtoExpert.new("comms.udp_kind_id_used_in_tcp",
    "Expected this type to be used only in UDP",
    expert.group.MALFORMED,
    expert.severity.ERROR);

local expert_bad_port_type = ProtoExpert.new("comms.bad_port_type", "Expected UDP or TCP Port",
    expert.group.MALFORMED,
    expert.severity.ERROR);

local expert_tcp_kind_id_used_in_udp = ProtoExpert.new("comms.tcp_kind_id_used_in_udp",
    "Expected this type to be used only in TCP",
    expert.group.MALFORMED,
    expert.severity.ERROR);

comms_protocol.fields = { kind_field, id_field, body_len_field, reply_to, padding, body }
comms_protocol.experts = { expert_bad_port_type, expert_unknown_kind, expert_udp_kind_id_used_in_tcp,
    expert_tcp_kind_id_used_in_udp }

local uint_to_kind_id_string = {
    [0] = "Associate UDP",
    [1] = "Associate UDP Accepted",
    [2] = "Finish Connect",
    [3] = "Audio From Client",
    [4] = "Keep Alive",
    [5] = "Audio To Client",
    [6] = "Finish Connect Accepted",
    [7] = "Error Response",
    [8] = "Hello",
    [9] = "User Joined",
    [10] = "User Left",
    [11] = "Disconnect",
    [12] = "P2P Synchronize",
    [13] = "P2P Acknowledge",
    [14] = "P2P Acknowledge Back",
    [15] = "P2P Established",
}

local message_kind_id_expected_port_type = {
    [0] = "UDP",
    [1] = "UDP",
    [2] = "TCP",
    [3] = "UDP",
    [4] = "TCP",
    [5] = "UDP",
    [6] = "TCP",
    [7] = "TCP",
    [8] = "TCP",
    [9] = "TCP",
    [10] = "TCP",
    [11] = "TCP",
    [12] = "UDP",
    [13] = "UDP",
    [14] = "UDP",
    [15] = "UDP",
}

local port_type_to_string = {
    [2] = "TCP",
    [3] = "UDP",
}

function comms_protocol.dissector(buffer, pinfo, tree)
    local length = buffer:len()
    if length == 0 then return end

    pinfo.cols.protocol = comms_protocol.name
    local kind_id = buffer(0, 1):uint()
    local package_kind_string = uint_to_kind_id_string[kind_id]
    local port_type = port_type_to_string[pinfo.port_type]

    local info_string
    if package_kind_string == nil then
        info_string = " (Unknown Package Kind)"
    else
        info_string = " (" .. package_kind_string .. ")"
        pinfo.cols.info:set(package_kind_string)
    end

    local subtree = tree:add(comms_protocol, buffer(),
        "Comms Package Data" .. info_string)

    if package_kind_string == nil then
        subtree:add_proto_expert_info(expert_unknown_kind)
    end

    if port_type == nil then
        subtree:add_proto_expert_info(expert_bad_port_type)
    elseif port_type == "UDP" and message_kind_id_expected_port_type[kind_id] == "TCP" then
        subtree:add_proto_expert_info(expert_tcp_kind_id_used_in_udp)
    elseif port_type == "TCP" and message_kind_id_expected_port_type[kind_id] == "UDP" then
        subtree:add_proto_expert_info(expert_udp_kind_id_used_in_tcp)
    end
    local header_subtree = subtree:add(comms_protocol, buffer(0, 24),
        "Header")
    header_subtree:add_le(kind_field, buffer(0, 1))
    header_subtree:add_le(id_field, buffer(1, 8))
    header_subtree:add_le(body_len_field, buffer(9, 2))

    if buffer(11, 1):uint() == 0 then
        header_subtree:add(reply_to, buffer(11, 1))
        header_subtree:add(padding, buffer(12, 12))
    else
        header_subtree:add(reply_to, buffer(11, 9))
        header_subtree:add(padding, buffer(20, 4))
    end
    subtree:add(body, buffer(24))

end

-- Add comms proto as option in "Decode As" -> ...
DissectorTable.get("udp.port"):add(0, comms_protocol)
DissectorTable.get("tcp.port"):add(0, comms_protocol)
